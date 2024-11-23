//! k23 virtual memory management
//!
//! ## Further reading
//!
//! This crate started out as a fork of the brilliant [`rmm`](https://gitlab.redox-os.org/redox-os/rmm)
//! crate. And while the two implementations have diverged quite a bit, the original codebase
//! is a great resource.
#![no_std]
#![no_main]
#![feature(used_with_arg)]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

// bring the test runner entry into scope
#[cfg(test)]
extern crate ktest as _;
// bring the #[panic_handler] and #[global_allocator] into scope
#[cfg(test)]
extern crate kernel as _;

mod alloc;
mod arch;
mod entry;
mod error;
mod flush;
mod mapper;
mod table;

use crate::entry::Entry;
use bitflags::Flags;
use core::fmt::Formatter;
use core::ops::Range;
use core::{cmp, fmt};

pub use alloc::{BitMapAllocator, BumpAllocator, FrameAllocator, FrameUsage};
pub use arch::*;
pub use error::Error;
pub use flush::Flush;
pub use mapper::Mapper;
pub use table::Table;

pub(crate) type Result<T> = core::result::Result<T, Error>;

pub trait Mode {
    type EntryFlags: Flags + From<usize> + Into<usize> + Copy + Clone + fmt::Debug;

    const VA_BITS: u32;
    const PAGE_SIZE: usize;

    /// The number of levels the page table has
    const PAGE_TABLE_LEVELS: usize;
    /// The number of page table entries in one table
    const PAGE_TABLE_ENTRIES: usize;

    /// Default flags for a valid page table leaf
    const ENTRY_FLAGS_LEAF: Self::EntryFlags;
    /// Default flags for a valid page table subtable entry
    const ENTRY_FLAGS_TABLE: Self::EntryFlags;
    /// Flags that mark something as read-execute
    const ENTRY_FLAGS_RX: Self::EntryFlags;
    /// Flags that mark something as read-only
    const ENTRY_FLAGS_RO: Self::EntryFlags;
    /// Flags that mark something as read-write
    const ENTRY_FLAGS_RW: Self::EntryFlags;

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

    /// Invalidate all address translation caches across all address spaces
    ///
    /// # Errors
    ///
    /// Should return an error if the underlying operation failed.
    fn invalidate_all() -> Result<()>;

    /// Invalidate address translation caches for the given `address_range` in the given `address_space`
    ///
    /// # Errors
    ///
    /// Should return an error if the underlying operation failed and the range could not be flushed.
    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> Result<()>;

    fn get_active_table(asid: usize) -> PhysicalAddress;
    fn activate_table(asid: usize, table: VirtualAddress);

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized;
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    #[must_use]
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    #[must_use]
    #[allow(clippy::cast_sign_loss)]
    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    #[must_use]
    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        assert!(!overflow, "physical address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        assert!(!overflow, "physical address underflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        assert!(!overflow, "physical address underflow");
        out
    }

    #[must_use]
    pub const fn as_raw(&self) -> usize {
        self.0
    }

    #[must_use]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(
            align.is_power_of_two(),
            "is_aligned_to: align is not a power-of-two"
        );

        self.as_raw() & (align - 1) == 0
    }

    #[must_use]
    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

    #[must_use]
    pub const fn align_up(self, alignment: usize) -> Self {
        Self((self.0 + alignment - 1) & !(alignment - 1))
    }
}

impl fmt::Display for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}

impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    #[must_use]
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    #[must_use]
    #[allow(clippy::cast_sign_loss)]
    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    #[must_use]
    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        assert!(!overflow, "virtual address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        assert!(!overflow, "virtual address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        assert!(!overflow, "virtual address underflow");
        out
    }

    #[must_use]
    pub const fn as_raw(&self) -> usize {
        self.0
    }

    #[must_use]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(
            align.is_power_of_two(),
            "is_aligned_to: align is not a power-of-two"
        );

        self.as_raw() & (align - 1) == 0
    }

    #[must_use]
    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

    #[must_use]
    pub const fn align_up(self, alignment: usize) -> Self {
        Self((self.0 + alignment - 1) & !(alignment - 1))
    }
}

impl fmt::Display for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}

impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

pub trait AddressRangeExt {
    fn is_aligned(&self, alignment: usize) -> bool;
    fn size(&self) -> usize;
    #[must_use]
    fn add(self, offset: usize) -> Self;
    #[must_use]
    fn concat(self, other: Self) -> Self;
}

impl AddressRangeExt for Range<PhysicalAddress> {
    fn is_aligned(&self, alignment: usize) -> bool {
        self.start.is_aligned(alignment) && self.end.is_aligned(alignment)
    }

    fn size(&self) -> usize {
        self.end.sub_addr(self.start)
    }

    fn add(self, offset: usize) -> Self {
        self.start.add(offset)..self.end.add(offset)
    }

    fn concat(self, other: Self) -> Self {
        Range {
            start: cmp::min(self.start, other.start),
            end: cmp::max(self.end, other.end),
        }
    }
}

impl AddressRangeExt for Range<VirtualAddress> {
    fn is_aligned(&self, alignment: usize) -> bool {
        self.start.is_aligned(alignment) && self.end.is_aligned(alignment)
    }

    fn size(&self) -> usize {
        self.end.sub_addr(self.start)
    }

    fn add(self, offset: usize) -> Self {
        self.start.add(offset)..self.end.add(offset)
    }

    fn concat(self, other: Self) -> Self {
        Range {
            start: cmp::min(self.start, other.start),
            end: cmp::max(self.end, other.end),
        }
    }
}

pub(crate) fn zero_frames<M: Mode>(mut ptr: *mut u64, num_frames: usize) {
    unsafe {
        let end = ptr.add((num_frames * M::PAGE_SIZE) / size_of::<u64>());
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }
}

pub(crate) fn phys_to_virt(
    physical_memory_offset: VirtualAddress,
    phys: PhysicalAddress,
) -> VirtualAddress {
    physical_memory_offset.add(phys.as_raw())
}
