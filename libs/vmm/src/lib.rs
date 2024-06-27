#![no_std]
#![feature(used_with_arg)]
#![no_main]

extern crate alloc as _;

mod alloc;
mod arch;
mod elf;
mod entry;
mod error;
mod flush;
mod mapper;
mod table;

use crate::entry::Entry;
use bitflags::Flags;
use core::fmt::Formatter;
use core::ops::Range;
use core::{fmt, mem};

pub use alloc::{BitMapAllocator, BumpAllocator, FrameAllocator, FrameUsage};
pub use arch::*;
#[cfg(feature = "elf")]
pub use elf::TlsTemplate;
pub use error::Error;
pub use flush::Flush;
pub use mapper::Mapper;
pub use table::Table;

pub(crate) type Result<T> = core::result::Result<T, Error>;

pub trait Mode {
    type EntryFlags: Flags + From<usize> + Into<usize> + Copy + Clone + fmt::Debug;

    const PHYS_OFFSET: usize;

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
    fn activate_table(asid: usize, table: VirtualAddress);

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized;

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        VirtualAddress::new(phys.as_raw()).add(Self::PHYS_OFFSET)
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
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

    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

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
    pub const fn new(bits: usize) -> Self {
        // debug_assert!(bits <= 0x0000_003f_ffff_ffff || bits > 0xffff_ffbf_ffff_ffff);
        debug_assert!(bits != 0);
        Self(bits)
    }

    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        if overflow {
            panic!("virtual address overflow");
        }
        Self(out)
    }

    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        if overflow {
            panic!("virtual address overflow");
        }
        Self(out)
    }

    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        if overflow {
            panic!("virtual address underflow");
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

    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

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
    fn add(self, offset: usize) -> Self;
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
}

pub(crate) fn zero_frames<M: Mode>(mut ptr: *mut u64, num_frames: usize) {
    unsafe {
        let end = ptr.add((num_frames * M::PAGE_SIZE) / mem::size_of::<u64>());
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }
}

/// `INIT` is a special `Mode` implementation that should be used *before* any memory mode is active
/// (i.e. no address translation is happening). It will wrap another `Mode` implementation and forward
/// functionality and properties to that inner implementation, **except** for the `Mode::phys_to_virt`
/// function which will always return and **identity translation** of the given physical address.
#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct INIT<M>(M);

impl<M> INIT<M> {
    pub fn into_inner(self) -> M {
        self.0
    }
}

impl<M: Mode> Mode for INIT<M> {
    type EntryFlags = M::EntryFlags;

    const PHYS_OFFSET: usize = 0;

    const PAGE_SIZE: usize = M::PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = M::PAGE_TABLE_LEVELS;
    const PAGE_TABLE_ENTRIES: usize = M::PAGE_TABLE_ENTRIES;

    const ENTRY_FLAGS_LEAF: Self::EntryFlags = M::ENTRY_FLAGS_LEAF;
    const ENTRY_FLAGS_TABLE: Self::EntryFlags = M::ENTRY_FLAGS_TABLE;
    const ENTRY_FLAGS_RX: Self::EntryFlags = M::ENTRY_FLAGS_RX;
    const ENTRY_FLAGS_RO: Self::EntryFlags = M::ENTRY_FLAGS_RO;
    const ENTRY_FLAGS_RW: Self::EntryFlags = M::ENTRY_FLAGS_RW;

    const ENTRY_ADDRESS_SHIFT: usize = M::ENTRY_ADDRESS_SHIFT;

    fn get_active_table(asid: usize) -> PhysicalAddress {
        M::get_active_table(asid)
    }

    fn activate_table(asid: usize, table: VirtualAddress) {
        M::activate_table(asid, table)
    }

    fn invalidate_all() -> crate::Result<()> {
        M::invalidate_all()
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        M::invalidate_range(asid, address_range)
    }

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized,
    {
        // Safety: INIT<M> has the same layout as M
        let entry: &Entry<M> = unsafe { mem::transmute(entry) };
        M::entry_is_leaf(entry)
    }
}

#[cfg(test)]
mod test {
    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        loop {}
    }
}
