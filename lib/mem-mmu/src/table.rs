// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::range::Range;

use mem_core::arch::{Arch, MapsAt, PageTableEntry, PageTableLevel};
use mem_core::{
    AddressRangeExt, AllocError, FrameAllocator, PageSize, PhysMap, PhysicalAddress, VirtualAddress,
};

use crate::utils::page_table_entries_for;

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

    /// Returns the virtual address of entry `index` in this table.
    pub(crate) fn entry_address(&self, index: u16, physmap: &PhysMap) -> VirtualAddress {
        let entry_phys = self
            .base
            .add(index as usize * size_of::<A::PageTableEntry>());
        physmap.phys_to_virt(entry_phys)
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
        let entry_virt = self.entry_address(index, physmap);

        // Safety: The address is always well aligned by the way we calculate it above (2.) we also
        // know `0` is a valid pattern for `A::PageTableEntry` and we know that we can access the
        // location either through the physmap or because we're still in bootstrapping.
        unsafe { arch.read(entry_virt) }
    }
}

impl<A: Arch> Table<A, marker::Owned> {
    /// Allocates a fresh, zeroed root page table from `frame_allocator`.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the frame allocator cannot provide a granule-sized,
    /// granule-aligned frame for the root table.
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

/// Visits page-table entries as [`visit`](Table::visit) walks the tree down to the
/// `S`-sized leaf level.
pub trait Visitor<A: Arch, S: PageSize> {
    /// Error type; the first `Err` a method returns aborts the walk and propagates
    /// out of it.
    type Error;

    /// Called on the way **down** for the interior entry at `index` in `table`.
    ///
    /// Returns the base address of the child table to descend into, or `None` to stop
    /// descending. The default is read-only: it descends into an existing table and
    /// stops at anything else, never writing the entry back.
    ///
    /// # Errors
    ///
    /// Any `Err` aborts the remainder of the walk.
    fn descend(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        index: u16,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<Option<PhysicalAddress>, Self::Error> {
        // Safety: the walk only descends through in-bounds indices.
        let entry = unsafe { table.get(index, physmap, arch) };

        // The default descent is read-only: it neither allocates nor writes. The
        // operations that use it (remap, set-attributes, unmap) run over an
        // already-mapped range, so a vacant interior entry means the range is not
        // fully mapped at `S` — a precondition violation.
        debug_assert!(
            !entry.is_vacant(),
            "virtual address range must be mapped at page size {}",
            S::BYTES,
        );

        if entry.is_table() {
            Ok(Some(entry.address()))
        } else {
            Ok(None)
        }
    }

    /// Called on the way **up** once the child table at `child_base`/`child_depth`,
    /// beneath the interior entry at `index` in `table`, has been fully visited.
    /// Defaults to a no-op.
    ///
    /// # Errors
    ///
    /// Any `Err` aborts the remainder of the walk.
    fn ascend(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        index: u16,
        child_base: PhysicalAddress,
        child_depth: u8,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<(), Self::Error> {
        let _ = (table, index, child_base, child_depth, physmap, arch);
        Ok(())
    }

    /// Called once for the contiguous run of `count` leaf entries starting at index
    /// `first` in `table`, whose first entry maps the `S`-sized page at `va` (the run
    /// spans `count` pages from there).
    ///
    /// # Errors
    ///
    /// Any `Err` aborts the remainder of the walk.
    fn fill(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        first: u16,
        count: u16,
        va: VirtualAddress,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<(), Self::Error>;
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

        let entry_virt = self.entry_address(index, physmap);

        // Safety: The address is always well aligned by the way we calculate it above (2.) we also
        // know `0` is a valid pattern for `A::PageTableEntry` and we know that we can access the
        // location either through the physmap or because we're still in bootstrapping.
        unsafe { arch.write(entry_virt, entry) }
    }

    /// Walks `range` from this table down to the `S`-sized leaf level, invoking
    /// `visitor` at each level.
    ///
    /// # Errors
    ///
    /// Propagates the first error `visitor` returns, aborting the walk.
    pub fn visit<S, V>(
        self,
        range: Range<VirtualAddress>,
        physmap: &PhysMap,
        arch: &A,
        visitor: &mut V,
    ) -> Result<(), V::Error>
    where
        S: PageSize,
        A: MapsAt<S>,
        V: Visitor<A, S>,
    {
        if range.len() == S::BYTES {
            // Optimized fast-path for single leaf-page operations such as when committing, decommitting, etc individual
            // CoW pages.
            visit_leaf::<S, A, V>(self, range.start, physmap, arch, visitor)
        } else {
            visit_range::<S, A, V>(self, range, physmap, arch, visitor)
        }
    }
}

fn visit_leaf<S, A, V>(
    mut table: Table<A, marker::Mut<'_>>,
    va: VirtualAddress,
    physmap: &PhysMap,
    arch: &A,
    visitor: &mut V,
) -> Result<(), V::Error>
where
    S: PageSize,
    A: MapsAt<S>,
    V: Visitor<A, S>,
{
    let depth = table.depth();
    let index = A::LEVELS[depth as usize].pte_index_of(va);

    if depth == <A as MapsAt<S>>::DEPTH {
        // Leaf table: fill the single entry.
        return visitor.fill(&mut table, index, 1, va, physmap, arch);
    }

    // The visitor hands back the child table to descend into, or `None` to stop.
    if let Some(child_base) = visitor.descend(&mut table, index, physmap, arch)? {
        // Safety: the visitor promised `child_base` is a table, so it sits one level
        // below `table`, and we inherit `table`'s mutable access to the tree.
        let subtable = unsafe { Table::from_raw_parts(child_base, depth + 1) };
        visit_leaf::<S, A, V>(subtable, va, physmap, arch, visitor)?;
        visitor.ascend(&mut table, index, child_base, depth + 1, physmap, arch)?;
    }

    Ok(())
}

fn visit_range<S, A, V>(
    mut table: Table<A, marker::Mut<'_>>,
    range: Range<VirtualAddress>,
    physmap: &PhysMap,
    arch: &A,
    visitor: &mut V,
) -> Result<(), V::Error>
where
    S: PageSize,
    A: MapsAt<S>,
    V: Visitor<A, S>,
{
    let depth = table.depth();

    if depth == <A as MapsAt<S>>::DEPTH {
        // Leaf table: the covered entries are one contiguous run.
        // Hand the whole run to `fill`.
        let level = &A::LEVELS[depth as usize];
        let first = level.pte_index_of(range.start);
        let last = level.pte_index_of(range.end.sub(1));
        return visitor.fill(
            &mut table,
            first,
            last - first + 1,
            range.start,
            physmap,
            arch,
        );
    }

    for (index, sub_range) in page_table_entries_for::<A>(range, &A::LEVELS[depth as usize]) {
        if let Some(child_base) = visitor.descend(&mut table, index, physmap, arch)? {
            // Safety: the visitor promised `child_base` is a table, so it sits one level
            // below `table`, and we inherit `table`'s mutable access to the tree.
            let subtable = unsafe { Table::from_raw_parts(child_base, depth + 1) };
            visit_range::<S, A, V>(subtable, sub_range, physmap, arch, visitor)?;
            visitor.ascend(&mut table, index, child_base, depth + 1, physmap, arch)?;
        }
    }

    Ok(())
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
