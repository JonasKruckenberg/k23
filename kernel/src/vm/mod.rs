// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod address_space;
mod address_space_region;
pub mod frame_alloc;
mod frame_list;
mod paged_vmo;
mod wired_vmo;
mod error;

use crate::machine_info::MachineInfo;
use crate::vm::frame_alloc::Frame;
pub use address_space::AddressSpace;
use alloc::format;
use alloc::string::ToString;
use core::fmt::Formatter;
use core::range::Range;
use core::{fmt, slice};
use loader_api::BootInfo;
use mmu::arch::PAGE_SIZE;
use mmu::{AddressRangeExt, Flush, VirtualAddress};
use paged_vmo::PagedVmo;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sync::{LazyLock, Mutex, OnceLock, RwLock};
use wired_vmo::WiredVmo;
use xmas_elf::program::Type;

const KERNEL_ASID: usize = 0;

pub static KERNEL_ASPACE: OnceLock<Mutex<AddressSpace>> = OnceLock::new();
static THE_ZERO_FRAME: LazyLock<Frame> = LazyLock::new(|| {
    let frame = frame_alloc::alloc_one_zeroed().unwrap();
    log::trace!("THE_ZERO_FRAME: {}", frame.addr());
    frame
});

pub fn init(boot_info: &BootInfo, minfo: &MachineInfo) -> crate::Result<()> {
    #[allow(tail_expr_drop_order)]
    KERNEL_ASPACE.get_or_try_init(|| -> crate::Result<_> {
        let (hw_aspace, mut flush) =
            mmu::AddressSpace::from_active(KERNEL_ASID, boot_info.physical_address_offset);

        let mut aspace = AddressSpace::from_active_kernel(
            hw_aspace,
            Some(ChaCha20Rng::from_seed(
                minfo.rng_seed.unwrap()[0..32].try_into().unwrap(),
            )),
        );

        reserve_wired_regions(&mut aspace, boot_info, &mut flush)?;
        flush.flush()?;

        for region in aspace.regions.iter() {
            log::trace!(
                "{:<40?} {}..{} {}",
                region.name,
                region.range.start,
                region.range.end,
                region.permissions
            )
        }

        Ok(Mutex::new(aspace))
    })?;

    Ok(())
}

fn reserve_wired_regions(
    aspace: &mut AddressSpace,
    boot_info: &BootInfo,
    flush: &mut Flush,
) -> crate::Result<()> {
    // reserve the physical memory map
    aspace.reserve(
        boot_info.physical_memory_map,
        Permissions::READ | Permissions::WRITE,
        Some("Physical Memory Map".to_string()),
        flush,
    )?;

    let own_elf = unsafe {
        let base = VirtualAddress::from_phys(
            boot_info.kernel_phys.start,
            boot_info.physical_address_offset,
        )
        .unwrap();

        slice::from_raw_parts(base.as_ptr(), boot_info.kernel_phys.size())
    };
    let own_elf = xmas_elf::ElfFile::new(own_elf).unwrap();

    for ph in own_elf.program_iter() {
        if ph.get_type().unwrap() != Type::Load {
            continue;
        }

        let virt = boot_info
            .kernel_virt
            .start
            .checked_add(ph.virtual_addr() as usize)
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

        aspace.reserve(
            Range {
                start: virt.align_down(PAGE_SIZE),
                end: virt
                    .checked_add(ph.mem_size() as usize)
                    .unwrap()
                    .checked_align_up(PAGE_SIZE)
                    .unwrap(),
            },
            permissions,
            Some(format!("Kernel {permissions} Segment")),
            flush,
        )?;
    }

    Ok(())
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
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}

impl PageFaultFlags {
    pub fn is_valid(&self) -> bool {
        self.contains(PageFaultFlags::LOAD) != self.contains(PageFaultFlags::STORE)
    }

    pub fn cause_is_read(&self) -> bool {
        self.contains(PageFaultFlags::LOAD)
    }
    pub fn cause_is_write(&self) -> bool {
        self.contains(PageFaultFlags::STORE)
    }
    pub fn cause_is_instr_fetch(&self) -> bool {
        self.contains(PageFaultFlags::INSTRUCTION)
    }
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct Permissions: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

impl fmt::Display for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
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

impl From<Permissions> for mmu::Flags {
    fn from(value: Permissions) -> Self {
        let mut out = mmu::Flags::empty();
        out.set(mmu::Flags::READ, value.contains(Permissions::READ));
        out.set(mmu::Flags::WRITE, value.contains(Permissions::WRITE));
        out.set(mmu::Flags::EXECUTE, value.contains(Permissions::EXECUTE));
        out
    }
}

impl Permissions {
    /// Returns whether the set of permissions is `R^X` ie doesn't allow
    /// write-execute at the same time.
    pub fn is_valid(&self) -> bool {
        !self.contains(Permissions::WRITE | Permissions::EXECUTE)
    }
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct Flags: u8 {
        const EAGER = 1 << 0;
    }
}


#[derive(Debug)]
pub enum Vmo {
    Wired(WiredVmo),
    Paged(RwLock<PagedVmo>),
}

impl Vmo {
    pub fn is_valid_offset(&self, offset: usize) -> bool {
        match self {
            Vmo::Wired(vmo) => vmo.is_valid_offset(offset),
            Vmo::Paged(vmo) => vmo.read().is_valid_offset(offset)
        }
    }
}