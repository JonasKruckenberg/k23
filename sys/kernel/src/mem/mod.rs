// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod address_space;
mod address_space_region;
pub mod bootstrap_alloc;
pub mod flush;
pub mod frame_alloc;
mod frame_list;
mod mmap;
mod provider;
mod trap_handler;
mod vmo;

use alloc::string::ToString;
use alloc::sync::Arc;
use core::fmt;
use core::num::NonZeroUsize;
use core::range::Range;

pub use address_space::{AddressSpace, Batch};
pub use address_space_region::AddressSpaceRegion;
pub use flush::Flush;
use loader_api::BootInfo;
use mem_core::{PhysicalAddress, VirtualAddress};
pub use mmap::Mmap;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use spin::{Mutex, OnceLock};
pub use trap_handler::handle_page_fault;
pub use vmo::Vmo;

use crate::arch;
use crate::mem::frame_alloc::FrameAllocator;

static KERNEL_ASPACE: OnceLock<Arc<Mutex<AddressSpace>>> = OnceLock::new();

pub fn with_kernel_aspace<F, R>(f: F) -> R
where
    F: FnOnce(&Arc<Mutex<AddressSpace>>) -> R,
{
    let aspace = KERNEL_ASPACE
        .get()
        .expect("kernel address space not initialized");
    f(aspace)
}

pub fn init(
    boot_info: &BootInfo,
    rand: &mut impl rand::Rng,
    frame_alloc: &'static FrameAllocator,
) -> crate::Result<()> {
    KERNEL_ASPACE.get_or_try_init(|| -> crate::Result<_> {
        let (hw_aspace, _flush) = arch::AddressSpace::from_active(arch::DEFAULT_ASID);

        // Safety: `init` is called during startup where the kernel address space is the only address space available
        let mut aspace = unsafe {
            AddressSpace::from_active_kernel(
                hw_aspace,
                Some(ChaCha20Rng::from_rng(rand)),
                frame_alloc,
            )
        };

        reserve_wired_regions(&mut aspace, boot_info);

        tracing::trace!("Kernel AddressSpace {aspace:?}");

        Ok(Arc::new(Mutex::new(aspace)))
    })?;

    Ok(())
}

fn reserve_wired_regions(aspace: &mut AddressSpace, boot_info: &BootInfo) {
    aspace
        .reserve(
            boot_info.physmap.range_virt(),
            Permissions::empty(), // reserved entries should never fault anyway
            Some("Physical Memory Map".to_string()),
        )
        .unwrap();

    // The kernel image's mem_size as reported by the loader is byte-sized; round up to a page
    // boundary so the reserve precondition holds.
    let kernel_virt = Range::from(
        boot_info.kernel_virt.start..boot_info.kernel_virt.end.align_up(arch::PAGE_SIZE),
    );
    aspace
        .reserve(
            kernel_virt,
            Permissions::empty(), // reserved entries should never fault anyway
            Some("Kernel Image".to_string()),
        )
        .unwrap();
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct PageFaultFlags: u8 {
        /// The fault was caused by a memory load
        const LOAD = 1 << 0;
        /// The fault was caused by a memory store
        const STORE = 1 << 1;
        /// The fault was caused by an instruction fetch
        const INSTRUCTION = 1 << 3;
    }
}

impl fmt::Display for PageFaultFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}

impl PageFaultFlags {
    pub fn is_valid(self) -> bool {
        !self.contains(PageFaultFlags::LOAD | PageFaultFlags::STORE)
    }

    pub fn cause_is_read(self) -> bool {
        self.contains(PageFaultFlags::LOAD)
    }
    pub fn cause_is_write(self) -> bool {
        self.contains(PageFaultFlags::STORE)
    }
    pub fn cause_is_instr_fetch(self) -> bool {
        self.contains(PageFaultFlags::INSTRUCTION)
    }
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct Permissions: u8 {
        /// Allow reads from the memory region
        const READ = 1 << 0;
        /// Allow writes to the memory region
        const WRITE = 1 << 1;
        /// Allow code execution from the memory region
        const EXECUTE = 1 << 2;
        /// Allow userspace to access the memory region
        const USER = 1 << 3;
    }
}

impl fmt::Display for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}

impl Permissions {
    /// Returns whether the set of permissions is `R^X` ie doesn't allow
    /// write-execute at the same time.
    pub fn is_valid(self) -> bool {
        !self.contains(Permissions::WRITE | Permissions::EXECUTE)
    }
}

impl From<PageFaultFlags> for Permissions {
    fn from(value: PageFaultFlags) -> Self {
        let mut out = Permissions::empty();
        if value.contains(PageFaultFlags::STORE) {
            out |= Permissions::WRITE;
        } else {
            out |= Permissions::READ;
        }
        if value.contains(PageFaultFlags::INSTRUCTION) {
            out |= Permissions::EXECUTE;
        }
        out
    }
}

pub trait ArchAddressSpace {
    type Flags: From<Permissions> + bitflags::Flags;

    fn new(asid: u16, frame_alloc: &FrameAllocator) -> crate::Result<(Self, Flush)>
    where
        Self: Sized;
    fn from_active(asid: u16) -> (Self, Flush)
    where
        Self: Sized;

    unsafe fn map_contiguous(
        &mut self,
        frame_alloc: &FrameAllocator,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: Self::Flags,
        flush: &mut Flush,
    ) -> crate::Result<()>;

    unsafe fn update_flags(
        &mut self,
        virt: VirtualAddress,
        len: NonZeroUsize,
        new_flags: Self::Flags,
        flush: &mut Flush,
    ) -> crate::Result<()>;

    unsafe fn unmap(
        &mut self,
        virt: VirtualAddress,
        len: NonZeroUsize,
        flush: &mut Flush,
    ) -> crate::Result<()>;

    unsafe fn query(&mut self, virt: VirtualAddress) -> Option<(PhysicalAddress, Self::Flags)>;

    unsafe fn activate(&self);

    fn new_flush(&self) -> Flush;
}
