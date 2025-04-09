// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod address;
mod address_space;
mod address_space_region;
pub mod bootstrap_alloc;
pub mod flush;
pub mod frame_alloc;
mod mmap;
mod provider;
mod trap_handler;
mod vmo;

use crate::arch;
use crate::mem::frame_alloc::FrameAllocator;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::num::NonZeroUsize;
use core::range::Range;
use core::{fmt, slice};
use loader_api::BootInfo;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use spin::{Mutex, OnceLock};
use xmas_elf::program::Type;

pub use address::{AddressRangeExt, PhysicalAddress, VirtualAddress};
pub use address_space::{AddressSpace, Batch};
pub use address_space_region::AddressSpaceRegion;
pub use flush::Flush;
pub use mmap::Mmap;
pub use trap_handler::handle_page_fault;
pub use vmo::Vmo;

pub const KIB: usize = 1024;
pub const MIB: usize = KIB * 1024;
pub const GIB: usize = MIB * 1024;

pub static KERNEL_ASPACE: OnceLock<Arc<Mutex<AddressSpace>>> = OnceLock::new();

pub fn with_kernel_aspace<F, R>(f: F) -> R
where
    F: FnOnce(&mut AddressSpace) -> R,
{
    let mut aspace = KERNEL_ASPACE
        .get()
        .expect("kernel address space not initialized")
        .lock();
    f(&mut aspace)
}

pub fn init(
    boot_info: &BootInfo,
    rand: &mut impl rand::RngCore,
    frame_alloc: &'static FrameAllocator,
) -> crate::Result<()> {
    KERNEL_ASPACE.get_or_try_init(|| -> crate::Result<_> {
        let (hw_aspace, mut flush) = arch::AddressSpace::from_active(arch::DEFAULT_ASID);

        // Safety: `init` is called during startup where the kernel address space is the only address space available
        let mut aspace = unsafe {
            AddressSpace::from_active_kernel(
                hw_aspace,
                Some(ChaCha20Rng::from_rng(rand)),
                frame_alloc,
            )
        };

        reserve_wired_regions(&mut aspace, boot_info, &mut flush);
        flush.flush().unwrap();

        log::trace!("Kernel AddressSpace {aspace:?}");

        Ok(Arc::new(Mutex::new(aspace)))
    })?;

    Ok(())
}

fn reserve_wired_regions(aspace: &mut AddressSpace, boot_info: &BootInfo, flush: &mut Flush) {
    // reserve the physical memory map
    aspace
        .reserve(
            Range::from(
                VirtualAddress::new(boot_info.physical_memory_map.start).unwrap()
                    ..VirtualAddress::new(boot_info.physical_memory_map.end).unwrap(),
            ),
            Permissions::READ | Permissions::WRITE,
            Some("Physical Memory Map".to_string()),
            flush,
        )
        .unwrap();

    // Safety: we have to trust the loaders BootInfo here
    let own_elf = unsafe {
        let base = boot_info
            .physical_address_offset
            .checked_add(boot_info.kernel_phys.start)
            .unwrap() as *const u8;

        slice::from_raw_parts(
            base,
            boot_info
                .kernel_phys
                .end
                .checked_sub(boot_info.kernel_phys.start)
                .unwrap(),
        )
    };
    let own_elf = xmas_elf::ElfFile::new(own_elf).unwrap();

    for ph in own_elf.program_iter() {
        if ph.get_type().unwrap() != Type::Load {
            continue;
        }

        let virt = VirtualAddress::new(boot_info.kernel_virt.start)
            .unwrap()
            .checked_add(usize::try_from(ph.virtual_addr()).unwrap())
            .unwrap();

        let mut permissions = Permissions::empty();
        if ph.flags().is_read() {
            permissions |= Permissions::READ;
        }
        if ph.flags().is_write() {
            permissions |= Permissions::WRITE;
        }
        if ph.flags().is_execute() {
            permissions |= Permissions::EXECUTE;
        }

        assert!(
            !permissions.contains(Permissions::WRITE | Permissions::EXECUTE),
            "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
            ph.virtual_addr(),
            ph.virtual_addr() + ph.mem_size()
        );

        aspace
            .reserve(
                Range {
                    start: virt.align_down(arch::PAGE_SIZE),
                    end: virt
                        .checked_add(usize::try_from(ph.mem_size()).unwrap())
                        .unwrap()
                        .checked_align_up(arch::PAGE_SIZE)
                        .unwrap(),
                },
                permissions,
                Some(format!("Kernel {permissions} Segment")),
                flush,
            )
            .unwrap();
    }
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
