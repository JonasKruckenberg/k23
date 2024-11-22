cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
    }
}

use crate::{PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::ops::Range;

bitflags! {
    #[derive(Debug, Copy, Clone)]
    pub struct ArchFlags: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

pub trait Arch {
    /// Number of usable bits in a virtual address
    const VA_BITS: u32;
    /// The smallest available page size
    const PAGE_SIZE: usize;

    /// The number of levels the page table has
    const PAGE_TABLE_LEVELS: usize;
    /// The number of page table entries in one table
    const PAGE_TABLE_ENTRIES: usize;

    /// On `RiscV` targets the entry's physical address bits are shifted 2 bits to the right.
    /// This constant is present to account for that, should be set to 0 on all other targets.
    const ENTRY_ADDRESS_SHIFT: usize = 0;

    // derived constants
    const PAGE_OFFSET_MASK: usize = Self::PAGE_SIZE - 1;
    /// Number of bits we need to shift an address by to reach the next page
    const PAGE_SHIFT: usize = (Self::PAGE_SIZE - 1).count_ones() as usize;
    /// Number of bits we need to shift an address by to reach the next page table entry
    const PAGE_ENTRY_SHIFT: usize = (Self::PAGE_TABLE_ENTRIES - 1).count_ones() as usize;
    const PAGE_ENTRY_MASK: usize = Self::PAGE_TABLE_ENTRIES - 1;

    fn map(
        &mut self,
        virt: VirtualAddress,
        phys: &mut dyn Iterator<Item = PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()>;
    fn map_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()>;
    fn remap_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()>;
    fn invalidate_all(&mut self) -> crate::Result<()>;
    fn invalidate_range(&mut self, asid: usize, range: Range<VirtualAddress>) -> crate::Result<()>;
    fn protect(&mut self, virt: Range<VirtualAddress>, flags: ArchFlags) -> crate::Result<()>;
    fn identity_map_contiguous(
        &mut self,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        let virt = VirtualAddress::new(phys.start.as_raw())..VirtualAddress::new(phys.end.as_raw());
        self.map_contiguous(virt, phys, flags)
    }
}
