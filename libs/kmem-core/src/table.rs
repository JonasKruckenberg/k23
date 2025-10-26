// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;

use crate::arch::PageTableEntry as _;
use crate::{Arch, PageTableLevel, PhysicalAddress};

/// A page table. Essentially a fixed-sized list of `A::PageTableEntry`s.
#[derive(Debug)]
pub struct Table<A: Arch, BorrowType> {
    base: PhysicalAddress,
    depth: u8,
    _marker: PhantomData<(A, BorrowType)>,
}

impl<A: Arch, BorrowType> Table<A, BorrowType> {
    /// Constructs a `Table` from its raw components: the base address and its depth the in the page table hierarchy.
    ///
    /// # Safety
    ///
    /// 1. The base address must indeed point to a page table.
    /// 2. The caller must make sure it has `BorrowType` access to the table memory.
    /// 3. The table must indeed be at the given depth in the hierarchy.
    pub(crate) const unsafe fn from_raw_parts(base: PhysicalAddress, depth: u8) -> Self {
        Self {
            base,
            depth,
            _marker: PhantomData,
        }
    }

    /// Returns the depth of this table in the page table hierarchy.
    ///
    /// `0` represents the root page table.
    pub(crate) const fn depth(&self) -> u8 {
        self.depth
    }

    /// Returns the [`PageTableLevel`] of this table, describing its layout and associated block size.
    pub(crate) fn level(&self, arch: &A) -> &'static PageTableLevel {
        &arch.memory_mode().levels()[self.depth as usize]
    }

    /// Returns the base address of this page table.
    pub const fn address(&self) -> PhysicalAddress {
        self.base
    }

    /// Returns `true` when _all_ page table entries in this table are _vacant_.
    pub fn is_empty(&self, arch: &A) -> bool {
        let mut is_empty = true;

        for entry_index in 0..self.level(arch).entries() {
            // Safety: we iterate through the entries for this level above, `entry_index` is always in-bounds
            let entry = unsafe { self.get(entry_index, arch) };

            is_empty |= entry.is_vacant();
        }

        is_empty
    }

    /// Returns the `A::PageTableEntry` at the given `index` without moving it. This leaves the entry
    /// unchanged.
    ///
    /// # Safety
    ///
    /// The caller must ensure `index` is in-bounds (less than the number of entries at this level).
    pub unsafe fn get(&self, index: usize, arch: &A) -> A::PageTableEntry {
        debug_assert!(index < self.level(arch).entries());

        let entry_phys = self.base.add(index * size_of::<A::PageTableEntry>());
        let entry_virt = arch.phys_to_virt(entry_phys);

        // Safety: The address is always well aligned by the way we calculate it above (2.) we also
        // know `0` is a valid pattern for `A::PageTableEntry` and we know that we can access the
        // location either through the physmap or because we're still in bootstrapping.
        unsafe { arch.read(entry_virt) }
    }
}

impl<A: Arch> Table<A, marker::Owned> {
    /// Returns an immutable reference to this `Table`
    pub fn borrow(&self) -> Table<A, marker::Immut<'_>> {
        Table {
            base: self.base,
            depth: self.depth,
            _marker: PhantomData,
        }
    }

    /// Returns a mutable reference to this `Table`
    pub fn borrow_mut(&mut self) -> Table<A, marker::Mut<'_>> {
        Table {
            base: self.base,
            depth: self.depth,
            _marker: PhantomData,
        }
    }
}

impl<A: Arch> Table<A, marker::Mut<'_>> {
    /// Returns a second mutable reference to this table.
    pub fn reborrow_mut(&mut self) -> Table<A, marker::Mut<'_>> {
        Table {
            base: self.base,
            depth: self.depth,
            _marker: PhantomData,
        }
    }

    /// Overrides the `A::PageTableEntry` at the given `index` without reading or dropping the old value.
    ///
    /// # Safety
    ///
    /// The caller must ensure `index` is in-bounds (less than the number of entries at this level).
    pub unsafe fn set(&mut self, index: usize, entry: A::PageTableEntry, arch: &A) {
        debug_assert!(index < self.level(arch).entries());

        let entry_phys = self.base.add(index * size_of::<A::PageTableEntry>());
        let entry_virt = arch.phys_to_virt(entry_phys);

        // Safety: The address is always well aligned by the way we calculate it above (2.) we also
        // know `0` is a valid pattern for `A::PageTableEntry` and we know that we can access the
        // location either through the physmap or because we're still in bootstrapping.
        unsafe { arch.write(entry_virt, entry) };
    }
}

impl<A: Arch> Clone for Table<A, marker::Immut<'_>> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A: Arch> Copy for Table<A, marker::Immut<'_>> {}

pub mod marker {
    use core::marker::PhantomData;

    #[derive(Debug)]
    pub enum Owned {}
    #[derive(Debug)]
    pub struct Mut<'a>(PhantomData<&'a mut ()>);
    #[derive(Debug)]
    pub struct Immut<'a>(PhantomData<&'a ()>);
}
