// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod address_space;
mod address_space_region;
pub mod frame_alloc;
mod mmap;
mod provider;
mod trap_handler;
mod vmo;

use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::ops::Range;
use core::{fmt, slice};

pub use address_space::{AddressSpace, Batch};
pub use address_space_region::AddressSpaceRegion;
use kmem_core::{
    AddressRangeExt, Flush, MemoryAttributes, WriteOrExecute,
};
use loader_api::BootInfo;
pub use mmap::Mmap;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use spin::{Mutex, OnceLock};
pub use trap_handler::handle_page_fault;
pub use vmo::Vmo;
use xmas_elf::program::{ProgramHeader, Type};

use crate::arch;
use crate::mem::frame_alloc::FrameAllocator;

pub const KIB: usize = 1024;
pub const MIB: usize = KIB * 1024;
pub const GIB: usize = MIB * 1024;

static KERNEL_ASPACE: OnceLock<Arc<Mutex<AddressSpace<arch::KmemArch>>>> = OnceLock::new();

pub fn with_kernel_aspace<F, R>(f: F) -> R
where
    F: FnOnce(&Arc<Mutex<AddressSpace<arch::KmemArch>>>) -> R,
{
    let aspace = KERNEL_ASPACE
        .get()
        .expect("kernel address space not initialized");
    f(aspace)
}

pub fn init<A: kmem_core::Arch>(
    boot_info: &BootInfo<A>,
    rand: &mut impl rand::RngCore,
    frame_alloc: &'static FrameAllocator,
) -> crate::Result<()> {
    KERNEL_ASPACE.get_or_try_init(|| -> crate::Result<_> {
        // let (hw_aspace, mut flush) = arch::AddressSpace::from_active(arch::DEFAULT_ASID);

        // Safety: `init` is called during startup where the kernel address space is the only address space available
        let mut aspace = unsafe {
            AddressSpace::new(
                boot_info.address_space,
                Some(ChaCha20Rng::from_rng(rand)),
                frame_alloc,
            )
        };

        let mut flush = Flush::new();

        reserve_wired_regions(&mut aspace, boot_info, &mut flush);
        flush.flush(aspace.raw.arch());

        tracing::trace!("Kernel AddressSpace {aspace:?}");

        Ok(Arc::new(Mutex::new(aspace)))
    })?;

    Ok(())
}

fn reserve_wired_regions<A: kmem_core::Arch>(
    aspace: &mut AddressSpace<A>,
    boot_info: &BootInfo<A>,
    flush: &mut Flush,
) {
    // reserve the physical memory map
    aspace
        .reserve(
            boot_info.physical_memory_map.clone(),
            MemoryAttributes::new()
                .with(MemoryAttributes::READ, true)
                .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write),
            Some("Physical Memory Map".to_string()),
            flush,
        )
        .unwrap();

    // Safety: we have to trust the loaders BootInfo here
    let own_elf = unsafe {
        let base = boot_info
            .address_space
            .arch()
            .phys_to_virt(boot_info.kernel_phys.start)
            .as_ptr();

        slice::from_raw_parts(base, boot_info.kernel_phys.len())
    };
    let own_elf = xmas_elf::ElfFile::new(own_elf).unwrap();

    for ph in own_elf.program_iter() {
        if ph.get_type().unwrap() != Type::Load {
            continue;
        }

        let virt = boot_info
            .kernel_virt
            .start
            .add(usize::try_from(ph.virtual_addr()).unwrap());

        let mut attributes = attributes_for_segment(&ph);

        aspace
            .reserve(
                Range {
                    start: virt.align_down(arch::PAGE_SIZE),
                    end: virt
                        .add(usize::try_from(ph.mem_size()).unwrap())
                        .align_up(arch::PAGE_SIZE),
                },
                attributes,
                Some(format!("Kernel {attributes} Segment")),
                flush,
            )
            .unwrap();
    }
}

fn attributes_for_segment(ph: &ProgramHeader) -> MemoryAttributes {
    MemoryAttributes::new()
        .with(MemoryAttributes::READ, ph.flags().is_read())
        .with(
            MemoryAttributes::WRITE_OR_EXECUTE,
            match (ph.flags().is_write(), ph.flags().is_execute()) {
                (false, false) => WriteOrExecute::Neither,
                (true, false) => WriteOrExecute::Write,
                (false, true) => WriteOrExecute::Execute,
                (true, true) => panic!(
                    "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
                    ph.virtual_addr(),
                    ph.virtual_addr() + ph.mem_size()
                ),
            },
        )
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
