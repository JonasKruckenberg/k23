cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
    }
    else {
        compile_error!("Unsupported architecture");
    }
}

use crate::vmm::{PhysicalAddress, VirtualAddress};
use bitflags::Flags;
use core::ops::Range;

pub trait Mode {
    type EntryFlags: Flags + From<usize> + Into<usize> + Copy + Clone;

    const PAGE_SIZE: usize;
    /// The offset at which to map the physical memory.
    ///
    /// The devices physical memory is mapped into kernel address space in its entirety
    /// so that physical->virtual address translations can be calculated by simply adding an offset.
    const PHYS_OFFSET: usize;

    /// The number of levels the page table has
    const PAGE_TABLE_LEVELS: usize;
    /// The number of page table entries in one table
    const PAGE_TABLE_ENTRIES: usize;

    /// Default flags for a valid page table leaf
    const ENTRY_FLAG_DEFAULT_LEAF: Self::EntryFlags;
    /// Default flags for a valid page table subtable entry
    const ENTRY_FLAG_DEFAULT_TABLE: Self::EntryFlags;
    /// On RiscV targets the entry's physical address bits are shifted 2 bits to the right.
    /// This constant is present to account for that, should be set to 0 on all other targets.
    const ENTRY_ADDRESS_SHIFT: usize = 0;

    // derived constants
    const PAGE_OFFSET_MASK: usize = Self::PAGE_SIZE - 1;
    /// Number of bits we need to shift an address by to reach the next page
    const PAGE_SHIFT: usize = (Self::PAGE_SIZE - 1).count_ones() as usize;
    /// Number of bits we need to shift an address by to reach the next page table entry
    const PAGE_ENTRY_SHIFT: usize = (Self::PAGE_TABLE_ENTRIES - 1).count_ones() as usize;
    const PAGE_ENTRY_MASK: usize = Self::PAGE_TABLE_ENTRIES - 1;

    /// Invalidate all address translation caches across all address spaces
    fn invalidate_all() -> crate::Result<()>;

    /// Invalidate address translation caches for the given `address_range` in the given `address_space`
    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()>;

    fn get_active_table(asid: usize) -> PhysicalAddress;
    fn activate_table(asid: usize, table: PhysicalAddress);

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        match phys.0.checked_add(Self::PHYS_OFFSET) {
            Some(some) => VirtualAddress(some),
            None => panic!("phys_to_virt({:?}) overflow", phys),
        }
    }
}
