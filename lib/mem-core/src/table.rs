// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::range::Range;

use arrayvec::ArrayVec;

use crate::arch::{Arch, PageTableEntry, PageTableLevel};
use crate::physmap::PhysMap;
use crate::utils::{PageTableEntries, page_table_entries_for};
use crate::{AllocError, FrameAllocator, PhysicalAddress, VirtualAddress};

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

    pub(crate) const fn into_raw_parts(self) -> (PhysicalAddress, u8) {
        (self.base, self.depth)
    }

    /// Returns the depth of this table in the page table hierarchy.
    ///
    /// `0` represents the root page table.
    pub(crate) const fn depth(&self) -> u8 {
        self.depth
    }

    /// Returns the [`PageTableLevel`] of this table, describing its layout and associated block size.
    pub(crate) fn level(&self) -> &'static PageTableLevel {
        &A::LEVELS[self.depth as usize]
    }

    /// Returns the base address of this page table.
    pub const fn address(&self) -> PhysicalAddress {
        self.base
    }

    /// Returns `true` when _all_ page table entries in this table are _vacant_.
    pub fn is_empty(&self, physmap: &PhysMap, arch: &A) -> bool {
        (0..self.level().entries()).all(|entry_index| {
            // Safety: we iterate through the entries for this level above, `entry_index` is always in-bounds
            unsafe { self.get(entry_index, physmap, arch) }.is_vacant()
        })
    }

    /// Returns the `A::PageTableEntry` at the given `index` without moving it. This leaves the entry
    /// unchanged.
    ///
    /// # Safety
    ///
    /// The caller must ensure `index` is in-bounds (less than the number of entries at this level).
    pub unsafe fn get(&self, index: u16, physmap: &PhysMap, arch: &A) -> A::PageTableEntry {
        let entry_phys = self
            .base
            .add(index as usize * size_of::<A::PageTableEntry>());

        let entry_virt = physmap.phys_to_virt(entry_phys);

        // Safety: The address is always well aligned by the way we calculate it above (2.) we also
        // know `0` is a valid pattern for `A::PageTableEntry` and we know that we can access the
        // location either through the physmap or because we're still in bootstrapping.
        unsafe { arch.read(entry_virt) }
    }
}

impl<A: Arch> Table<A, marker::Owned> {
    pub fn allocate(
        frame_allocator: impl FrameAllocator,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<Self, AllocError> {
        let base = frame_allocator.allocate_contiguous_zeroed(A::GRANULE_LAYOUT, physmap, arch)?;

        Ok(Self {
            base,
            depth: 0,
            _marker: PhantomData,
        })
    }

    pub fn deallocate(self, frame_allocator: impl FrameAllocator) {
        // Safety: page tables are always exactly one page translation granule in size
        unsafe { frame_allocator.deallocate(self.base, A::GRANULE_LAYOUT) };
    }

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
    pub unsafe fn set(
        &mut self,
        index: u16,
        entry: A::PageTableEntry,
        physmap: &PhysMap,
        arch: &A,
    ) {
        debug_assert!(index < self.level().entries());

        let entry_phys = self
            .base
            .add(index as usize * size_of::<A::PageTableEntry>());

        let entry_virt = physmap.phys_to_virt(entry_phys);

        // Safety: The address is always well aligned by the way we calculate it above (2.) we also
        // know `0` is a valid pattern for `A::PageTableEntry` and we know that we can access the
        // location either through the physmap or because we're still in bootstrapping.
        unsafe { arch.write(entry_virt, entry) }
    }

    pub fn visit_mut<F, E>(
        self,
        range: Range<VirtualAddress>,
        physmap: &PhysMap,
        arch: &A,
        mut visit_entry: F,
    ) -> Result<(), E>
    where
        Self: Sized,
        F: FnMut(
            &mut A::PageTableEntry,
            Range<VirtualAddress>,
            &'static PageTableLevel,
        ) -> Result<(), E>,
    {
        struct Level<'t, A>
        where
            A: Arch,
        {
            table: Table<A, marker::Mut<'t>>,
            entries_iter: PageTableEntries<A>,
        }

        // NB: we use a fixed size stack here to help with loop unrolling and
        // enforce a known upper bound on the runtime-complexity of this function
        // (5 level deep max). 5 is chosen because it is the deepest page table depth
        // across all our supported target architectures.
        let mut stack: ArrayVec<Level<'_, _>, 5> = ArrayVec::from_iter([Level {
            table: self,
            entries_iter: page_table_entries_for(range, &A::LEVELS[0]),
        }]);

        // Depth-first, in-order walk of the page tables.
        while let Some(frame) = stack.last_mut() {
            let Some((entry_index, range)) = frame.entries_iter.next() else {
                // This table is fully visited; backtrack to its parent.
                stack.pop();
                continue;
            };

            // Safety: `page_table_entries_for` yields only in-bound indices
            let mut entry = unsafe { frame.table.get(entry_index, physmap, arch) };

            visit_entry(&mut entry, range, frame.table.level())?;

            // Safety: `page_table_entries_for` yields only in-bound indices
            unsafe {
                frame.table.set(entry_index, entry, physmap, arch);
            }

            if entry.is_table() {
                debug_assert!((frame.table.depth() as usize + 1) < A::LEVELS.len());

                // Safety: We checked the entry is a table above (1.) know the depth is correct (2.)
                // and inherit the mutable access from self.
                let subtable: Table<_, marker::Mut<'_>> = unsafe {
                    Table::from_raw_parts(
                        entry.address(),
                        frame.table.depth().checked_add(1).unwrap(),
                    )
                };

                // Descend at once, before advancing to the next sibling, so entries
                // are visited in ascending address order.
                stack.push(Level {
                    entries_iter: page_table_entries_for(range, subtable.level()),
                    table: subtable,
                });
            }
        }

        Ok(())
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

#[cfg(test)]
mod tests {
    use std::alloc::Layout;

    use proptest::prelude::*;

    use super::*;
    use crate::test_utils::{Machine, MachineBuilder};
    use crate::{MemoryAttributes, for_arch};

    for_arch!(A in [
        Riscv64Sv39,
        #[cfg(not(miri))]
        Riscv64Sv48,
        #[cfg(not(miri))]
        Riscv64Sv57,
    ] {
        proptest! {
            /// Regression test for [`Table::is_empty`] (review Blocker: `|=` should be `&=`).
            ///
            /// `is_empty` must return `true` exactly when every entry is vacant. The buggy
            /// `|=` accumulation makes it unconditionally report `true`.
            #[test]
            fn is_empty_iff_all_entries_vacant(
                occupied in proptest::collection::hash_set(0u16..A::LEVELS[0].entries(), 0..32),
            ) {
                let machine: Machine<A> = MachineBuilder::new()
                    .with_memory_regions([
                        Layout::from_size_align(0x20000, A::GRANULE_SIZE).unwrap()
                    ])
                    .finish();

                let (address_space, frame_allocator, physmap) =
                    machine.bootstrap_address_space(A::DEFAULT_PHYSMAP_BASE);
                let arch = address_space.arch();

                let mut table =
                    Table::allocate(frame_allocator.by_ref(), &physmap, arch).unwrap();

                // Occupy the chosen entries with leaves. The leaf address is irrelevant —
                // `is_empty` only inspects each entry's vacancy.
                let leaf = <<A as Arch>::PageTableEntry as PageTableEntry>::new_leaf(
                    PhysicalAddress::new(A::GRANULE_SIZE),
                    MemoryAttributes::new().with(MemoryAttributes::READ, true),
                );
                for &index in &occupied {
                    // Safety: `index` is in `0..A::LEVELS[0].entries()`, in-bounds for the root table.
                    unsafe {
                        table.borrow_mut().set(index, leaf, &physmap, arch);
                    }
                }

                prop_assert_eq!(
                    table.borrow().is_empty(&physmap, arch),
                    occupied.is_empty(),
                );
            }
        }
    });
}
