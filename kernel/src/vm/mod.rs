mod address_space;
mod address_space_region;
pub mod frame_alloc;
mod frame_list;
mod paged_vmo;
mod wired_vmo;

use crate::vm::frame_alloc::Frame;
pub use address_space::AddressSpace;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::alloc::Layout;
use core::range::Range;
use core::{fmt, iter, slice};
use loader_api::BootInfo;
use mmu::arch::PAGE_SIZE;
use mmu::{AddressRangeExt, VirtualAddress};
use paged_vmo::PagedVmo;
use sync::{LazyLock, Mutex};
use wired_vmo::WiredVmo;
use xmas_elf::program::Type;

const KERNEL_ASID: usize = 0;

static THE_ZERO_FRAME: LazyLock<Frame> = LazyLock::new(|| frame_alloc::alloc_one_zeroed().unwrap());

pub fn test(boot_info: &BootInfo) -> crate::Result<()> {
    let (hw_aspace, _) =
        mmu::AddressSpace::from_active(KERNEL_ASID, boot_info.physical_address_offset);

    let mut aspace = AddressSpace::new_kernel(hw_aspace, None);
    reserve_wired_regions(&mut aspace, boot_info)?;

    for region in aspace.regions.iter() {
        log::trace!(
            "{:<40} {}..{} {}",
            region.name,
            region.range.start,
            region.range.end,
            region.permissions
        )
    }

    let layout = Layout::from_size_align(4 * PAGE_SIZE, PAGE_SIZE).unwrap();

    let vmo = Arc::new(Vmo::Paged(Mutex::new(PagedVmo::from_iter(iter::repeat_n(
        THE_ZERO_FRAME.clone(),
        layout.size() / PAGE_SIZE,
    )))));

    let range = aspace
        .map(layout, vmo, 0, Permissions::READ, "Test".to_string())?
        .range;

    aspace
        .page_fault(
            range.start.checked_add(3 * PAGE_SIZE).unwrap(),
            PageFaultFlags::WRITE,
        )
        .unwrap();

    Ok(())
}

fn reserve_wired_regions(aspace: &mut AddressSpace, boot_info: &BootInfo) -> crate::Result<()> {
    // reserve the physical memory map
    aspace.reserve(
        boot_info.physical_memory_map,
        Permissions::READ | Permissions::WRITE,
        "Physical Memory Map".to_string(),
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
            format!("Kernel {permissions} Segment"),
        )?;
    }

    Ok(())
}

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct PageFaultFlags: u8 {
        /// The fault was caused by a memory read
        const READ = 1 << 0;
        /// The fault was caused by a memory write
        const WRITE = 1 << 1;
        /// The fault was caused by an instruction fetch
        const INSTRUCTION = 1 << 3;
    }
}

impl PageFaultFlags {
    pub fn is_valid(&self) -> bool {
        self.contains(PageFaultFlags::READ) != self.contains(PageFaultFlags::WRITE)
    }

    pub fn cause_is_read(&self) -> bool {
        self.contains(PageFaultFlags::READ)
    }
    pub fn cause_is_write(&self) -> bool {
        self.contains(PageFaultFlags::WRITE)
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
        if value.contains(PageFaultFlags::WRITE) {
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

#[derive(Debug)]
pub enum Vmo {
    Wired(WiredVmo),
    Paged(Mutex<PagedVmo>),
}
