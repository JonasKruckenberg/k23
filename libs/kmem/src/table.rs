use core::marker::PhantomData;
use core::ops::Range;

use arrayvec::ArrayVec;

use crate::arch::{Arch, PageTableEntry, PageTableLevel};
use crate::physmap::PhysicalMemoryMapping;
use crate::utils::{page_table_entries_for, PageTableEntries};
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
    pub fn is_empty(&self, physmap: &PhysicalMemoryMapping, arch: &A) -> bool {
        let mut is_empty = true;

        for entry_index in 0..self.level().entries() {
            // Safety: we iterate through the entries for this level above, `entry_index` is always in-bounds
            let entry = unsafe { self.get(entry_index, physmap, arch) };

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
    pub unsafe fn get(
        &self,
        index: u16,
        physmap: &PhysicalMemoryMapping,
        arch: &A,
    ) -> A::PageTableEntry {
        let entry_phys = self
            .base
            .add(index as usize * size_of::<A::PageTableEntry>());

        physmap.with_mapped(entry_phys, |entry_virt| {
            // Safety: The address is always well aligned by the way we calculate it above (2.) we also
            // know `0` is a valid pattern for `A::PageTableEntry` and we know that we can access the
            // location either through the physmap or because we're still in bootstrapping.
            unsafe { arch.read(entry_virt) }
        })
    }
}

impl<A: Arch> Table<A, marker::Owned> {
    pub fn allocate(
        frame_allocator: impl FrameAllocator,
        physmap: &PhysicalMemoryMapping,
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
        physmap: &PhysicalMemoryMapping,
        arch: &A,
    ) {
        debug_assert!(index < self.level().entries());

        let entry_phys = self
            .base
            .add(index as usize * size_of::<A::PageTableEntry>());

        physmap.with_mapped(entry_phys, |entry_virt| {
            // Safety: The address is always well aligned by the way we calculate it above (2.) we also
            // know `0` is a valid pattern for `A::PageTableEntry` and we know that we can access the
            // location either through the physmap or because we're still in bootstrapping.
            unsafe { arch.write(entry_virt, entry) }
        });
    }

    pub fn visit_mut<F, E>(
        self,
        range: Range<VirtualAddress>,
        physmap: &PhysicalMemoryMapping,
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

        let mut stack: ArrayVec<Level<'_, _>, 5> = ArrayVec::from_iter([Level {
            table: self,
            entries_iter: page_table_entries_for(range.clone(), &A::LEVELS[0]),
        }]);

        while let Some(mut frame) = stack.pop() {
            for (entry_index, range) in frame.entries_iter {
                let mut entry = unsafe { frame.table.get(entry_index, physmap, arch) };

                visit_entry(&mut entry, range.clone(), frame.table.level())?;

                unsafe {
                    frame.table.set(entry_index, entry, physmap, arch);
                }

                if entry.is_table() {
                    // Safety: We checked the entry is a table above (1.) know the depth is correct (2.).
                    let subtable: Table<_, marker::Mut<'_>> =
                        unsafe { Table::from_raw_parts(entry.address(), frame.table.depth() + 1) };

                    // Push new frame for subtable
                    stack.push(Level {
                        entries_iter: page_table_entries_for(range, subtable.level()),
                        table: subtable,
                    });
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
