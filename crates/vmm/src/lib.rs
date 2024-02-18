#![cfg_attr(not(test), no_std)]
#![feature(error_in_core)]

use bitflags::Flags;
use core::fmt;
use core::fmt::Formatter;
use core::ops::Range;

mod alloc;
mod arch;
mod entry;
mod error;
mod flush;
mod mapper;
mod table;

use crate::entry::Entry;
pub use alloc::{BitMapAllocator, BumpAllocator, FrameAllocator, FrameUsage};
pub use arch::*;
pub use error::Error;
pub use flush::Flush;
pub use mapper::Mapper;

pub(crate) type Result<T> = core::result::Result<T, Error>;

pub trait Mode {
    type EntryFlags: Flags + From<usize> + Into<usize> + Copy + Clone + fmt::Debug;

    const PAGE_SIZE: usize;

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
    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> Result<()>;

    fn get_active_table(asid: usize) -> PhysicalAddress;
    fn activate_table(asid: usize, table: PhysicalAddress);

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized;
}

#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const unsafe fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        if overflow {
            panic!("physical address overflow");
        }
        Self(out)
    }

    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        if overflow {
            panic!("physical address underflow");
        }
        Self(out)
    }

    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        if overflow {
            panic!("physical address underflow");
        }
        out
    }

    pub const fn as_raw(&self) -> usize {
        self.0
    }

    pub const fn is_aligned(&self, align: usize) -> bool {
        if !align.is_power_of_two() {
            panic!("is_aligned_to: align is not a power-of-two");
        }

        self.as_raw() & (align - 1) == 0
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

#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    pub const unsafe fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        if overflow {
            panic!("physical address overflow");
        }
        Self(out)
    }

    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        if overflow {
            panic!("physical address underflow");
        }
        Self(out)
    }

    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        if overflow {
            panic!("physical address underflow");
        }
        out
    }

    pub const fn as_raw(&self) -> usize {
        self.0
    }

    pub const fn is_aligned(&self, align: usize) -> bool {
        if !align.is_power_of_two() {
            panic!("is_aligned_to: align is not a power-of-two");
        }

        self.as_raw() & (align - 1) == 0
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
