cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
    } else if #[cfg(target_arch = "aarch64")] {
        mod aarch64;
        pub use aarch64::*;
    }
}

use crate::{PhysicalAddress, VirtualAddress};
use core::fmt;
use core::ops::Range;

pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;

pub trait Arch {
    type PageTableEntry: PageTableEntry + fmt::Debug;

    /// Number of usable bits in a virtual address
    const VIRT_ADDR_BITS: u32;
    /// The number of levels the page table has
    const PAGE_TABLE_LEVELS: usize;
    /// The number of page table entries in one table
    const PAGE_TABLE_ENTRIES: usize = 1 << Self::PAGE_ENTRY_SHIFT;
    /// Number of bits we need to shift an address by to reach the next page table entry
    const PAGE_ENTRY_SHIFT: usize;

    /// Return whether the combination of `virt`,`phys`, and `remaining_bytes` can be mapped at the given `level`.
    fn can_map_at_level(
        virt: VirtualAddress,
        phys: PhysicalAddress,
        remaining_bytes: usize,
        level: usize,
    ) -> bool;

    /// Return the page size for the given page table level.
    fn page_size_for_level(level: usize) -> usize;

    /// Parse the `level` page table entry index from the given virtual address.
    fn pte_index_for_level(virt: VirtualAddress, level: usize) -> usize;

    /// Invalidate all address translation caches across all address spaces.
    ///
    /// # Errors
    ///
    /// Should return an error if the underlying operation failed.
    fn invalidate_all() -> crate::Result<()>;

    /// Invalidate address translation caches for the given `address_range` in the given `address_space`.
    ///
    /// # Errors
    ///
    /// Should return an error if the underlying operation failed and the range could not be flushed.
    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()>;

    /// Return a pointer to the currently active page table.
    fn get_active_pgtable(_asid: usize) -> PhysicalAddress;

    /// Set the given page table as the currently active page table.
    ///
    /// # Safety
    ///
    /// This will invalidate pointers if not used carefully
    unsafe fn activate_pgtable(asid: usize, pgtable: PhysicalAddress);
}

pub trait PageTableEntry {
    type Flags: bitflags::Flags + From<crate::Flags> + Copy + fmt::Debug;

    /// Flag(s) that represent a valid page table entry.
    const FLAGS_VALID: Self::Flags;
    /// Flags representing a read/write/execute page table entry, for masking purposes.
    const FLAGS_RWX: Self::Flags;

    /// Return whether this page table entry is valid, aka not vacant.
    fn is_valid(&self) -> bool;
    /// Return whether this page table entry is a leaf, ie its address points to a physical frame that
    /// is mapped into virtual memory instead of pointing to another page table.
    fn is_leaf(&self) -> bool;

    /// Replace this page table entries address and flags.
    fn replace_address_and_flags(&mut self, addr: PhysicalAddress, flags: Self::Flags);
    /// Return this page table entries address and flags.
    fn get_address_and_flags(&self) -> (PhysicalAddress, Self::Flags);
    /// Clear this page table entry.
    fn clear(&mut self);
}
