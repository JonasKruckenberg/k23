// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::range::Range;

use arrayvec::ArrayVec;
use mem_core::arch::{Arch, MAX_PAGE_TABLE_LEVELS, PageTableEntry, PageTableLevel};
use mem_core::{AllocError, FrameAllocator, PhysMap, PhysicalAddress, VirtualAddress};

use crate::utils::{PageTableEntries, page_table_entries_for};

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

/// A point in a depth-first page-table walk at which [`visit_mut`](Table::visit_mut)
/// invokes its visitor.
#[derive(Debug)]
pub enum Step {
    /// On the way **down**: an entry covering `range`, in the table at `depth`
    /// (root = `0`). Mutating the entry into a table makes the walk descend into
    /// it; leaving it a leaf or vacant does not.
    Descend {
        range: Range<VirtualAddress>,
        depth: u8,
    },
    /// On the way **up**: the subtable beneath this entry — itself at `child_depth`
    /// — has just been fully visited. The point at which a now-empty subtable is
    /// reclaimed and its entry vacated.
    Ascend { child_depth: u8 },
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

    /// Depth-first, in-order walk of every page-table entry spanning `range`.
    ///
    /// `visit` is called for each entry on the way **down** with [`Step::Descend`],
    /// and once on the way **up** with [`Step::Ascend`] for every entry the walk
    /// descended into, after that entry's whole subtable has been visited. The
    /// (possibly mutated) entry is written back after each call, and the walk
    /// descends into any entry a [`Step::Descend`] visit leaves as a table.
    ///
    /// A single visitor handles both steps so it can own the mutable state — e.g. a
    /// [`Flush`][crate::Flush] — that both the descend and ascend phases touch.
    ///
    /// # Errors
    ///
    /// Propagates the first error returned by `visit`, aborting the remainder of the
    /// walk.
    ///
    /// # Panics
    ///
    /// Panics if the page-table depth would overflow while descending (unreachable for the
    /// architectures k23 supports, whose depth is bounded by `A::LEVELS`).
    pub fn visit_mut<F, E>(
        self,
        range: Range<VirtualAddress>,
        physmap: &PhysMap,
        arch: &A,
        mut visit: F,
    ) -> Result<(), E>
    where
        Self: Sized,
        F: FnMut(&mut A::PageTableEntry, Step) -> Result<(), E>,
    {
        struct Frame<'t, A>
        where
            A: Arch,
        {
            table: Table<A, marker::Mut<'t>>,
            entries_iter: PageTableEntries<A>,
            /// Index in the parent table of the entry descended through, and that
            /// entry's value. Written back — after its [`Step::Ascend`] visit — only
            /// once this subtable is fully visited, so a visitor can vacate it
            /// post-order. Both ignored for the root frame.
            parent_index: u16,
            parent_entry: A::PageTableEntry,
        }

        // NB: a fixed-capacity stack keeps this walk iterative and bounds its runtime
        // complexity to the page-table depth, which never exceeds `MAX_PAGE_TABLE_LEVELS`.
        let mut stack: ArrayVec<Frame<'_, A>, MAX_PAGE_TABLE_LEVELS> = ArrayVec::new();
        stack.push(Frame {
            entries_iter: page_table_entries_for(range, &A::LEVELS[0]),
            table: self,
            parent_index: 0,
            parent_entry: A::PageTableEntry::VACANT,
        });

        // Depth-first, in-order walk of the page tables.
        while let Some(frame) = stack.last_mut() {
            let Some((entry_index, range)) = frame.entries_iter.next() else {
                // This subtable is fully visited; ascend to its parent, letting the
                // visitor reclaim it, then write the (possibly vacated) parent entry.
                let mut done = stack.pop().unwrap();
                if let Some(parent) = stack.last_mut() {
                    visit(
                        &mut done.parent_entry,
                        Step::Ascend {
                            child_depth: done.table.depth(),
                        },
                    )?;

                    // Safety: `parent_index` indexed `parent` on the way down, so it
                    // is in-bounds.
                    unsafe {
                        parent
                            .table
                            .set(done.parent_index, done.parent_entry, physmap, arch);
                    }
                }
                continue;
            };

            // Safety: `page_table_entries_for` yields only in-bound indices
            let mut entry = unsafe { frame.table.get(entry_index, physmap, arch) };

            visit(
                &mut entry,
                Step::Descend {
                    range,
                    depth: frame.table.depth(),
                },
            )?;

            if entry.is_table() {
                debug_assert!((frame.table.depth() as usize + 1) < A::LEVELS.len());

                // Safety: We checked the entry is a table above, the depth is one below
                // this frame, and we inherit the mutable access from self.
                let subtable: Table<_, marker::Mut<'_>> = unsafe {
                    Table::from_raw_parts(
                        entry.address(),
                        frame.table.depth().checked_add(1).unwrap(),
                    )
                };

                // Descend at once, before advancing to the next sibling, so entries are
                // visited in ascending address order. This entry is written back when
                // its subtable is fully visited (the `Ascend` arm above).
                stack.push(Frame {
                    entries_iter: page_table_entries_for(range, subtable.level()),
                    table: subtable,
                    parent_index: entry_index,
                    parent_entry: entry,
                });
            } else {
                // Leaf or vacant: nothing to descend into, so write the entry now.
                // Safety: `page_table_entries_for` yields only in-bound indices
                unsafe {
                    frame.table.set(entry_index, entry, physmap, arch);
                }
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
