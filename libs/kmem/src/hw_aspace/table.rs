// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::{mem, slice};

use arrayvec::ArrayVec;

use crate::arch::{Arch, PageTableEntry as _, PageTableLevel};
use crate::{AllocError, FrameAllocator, MemoryAttributes, PhysicalAddress, VirtualAddress};

pub struct Table<BorrowType, A> {
    entries: PhysicalAddress,
    _marker: PhantomData<(BorrowType, A)>,
}

pub struct Root<A>(Table<marker::Owned, A>);

pub struct Cursor<BorrowType, A> {
    stack: ArrayVec<Table<BorrowType, A>, 8>,
    virt: VirtualAddress,
    level: u32,
}

// ===== impl *Any* Table =====

impl<BorrowType, A: Arch> Table<BorrowType, A> {
    pub unsafe fn cast_owned(self) -> Table<marker::Owned, A> {
        Table {
            entries: self.entries,
            _marker: PhantomData,
        }
    }

    pub fn is_empty(&self, lvl: &'static PageTableLevel) -> bool {
        let entries = unsafe {
            slice::from_raw_parts(
                self.entries.as_ptr().cast::<A::PageTableEntry>(),
                lvl.entries(),
            )
        };

        entries.iter().any(|entry| entry.is_vacant())
    }

    // pub fn address(&self) -> PhysicalAddress {
    //     self.entries
    // }

    pub fn get(&self, index: usize) -> &A::PageTableEntry {
        unsafe { self.entry_raw(index).as_ref() }
    }

    fn entry_raw(&self, index: usize) -> NonNull<A::PageTableEntry> {
        unsafe {
            self.entries
                .as_non_null()
                .unwrap()
                .cast::<A::PageTableEntry>()
                .add(index)
        }
    }

    unsafe fn from_raw(entries: PhysicalAddress) -> Self {
        Self {
            entries,
            _marker: PhantomData,
        }
    }
}

// ===== impl *Owned* Table =====

impl<A: Arch> Table<marker::Owned, A> {
    pub fn allocate<F: FrameAllocator<A>>(frame_allocator: F) -> Result<Self, AllocError> {
        let layout = unsafe { Layout::from_size_align_unchecked(A::PAGE_SIZE, A::PAGE_SIZE) };

        let entries = frame_allocator.allocate_contiguous_zeroed(layout)?;

        Ok(unsafe { Table::from_raw(entries) })
    }

    pub unsafe fn deallocate<F: FrameAllocator<A>>(self, frame_allocator: F) {
        let layout = unsafe { Layout::from_size_align_unchecked(A::PAGE_SIZE, A::PAGE_SIZE) };

        unsafe { frame_allocator.deallocate(self.entries, layout) }
    }

    pub fn borrow_mut(&mut self) -> Table<marker::Mut<'_>, A> {
        unsafe { Table::from_raw(self.entries) }
    }

    pub fn borrow(&self) -> Table<marker::Immut<'_>, A> {
        unsafe { Table::from_raw(self.entries) }
    }
}

// ===== impl *Mutable* Table =====

impl<A: Arch> Table<marker::Mut<'_>, A> {
    pub fn get_mut(&mut self, index: usize) -> &mut A::PageTableEntry {
        unsafe { self.entry_raw(index).as_mut() }
    }

    pub fn insert_table(
        &mut self,
        index: usize,
        table: Table<marker::Owned, A>,
    ) -> A::PageTableEntry {
        mem::replace(
            self.get_mut(index),
            A::PageTableEntry::new_table(table.entries),
        )
    }

    pub fn insert_leaf(
        &mut self,
        index: usize,
        block_address: PhysicalAddress,
        attributes: MemoryAttributes,
    ) -> A::PageTableEntry {
        mem::replace(
            self.get_mut(index),
            A::PageTableEntry::new_leaf(block_address, attributes),
        )
    }

    pub fn remove(&mut self, index: usize) -> A::PageTableEntry {
        mem::replace(self.get_mut(index), A::PageTableEntry::new_empty())
    }
}

// ===== impl Root =====

impl<A: Arch> Root<A> {
    pub fn from_owned(table: Table<marker::Owned, A>) -> Self {
        Self(table)
    }

    pub fn address(&self) -> PhysicalAddress {
        self.0.entries
    }

    pub fn cursor_for(&self, idx: VirtualAddress) -> Cursor<marker::Immut<'_>, A> {
        let mut stack = ArrayVec::new();
        stack.push(self.0.borrow());

        Cursor {
            stack,
            virt: idx,
            level: (A::PAGE_TABLE_LEVELS.len() - 1) as u32,
        }
    }

    pub fn cursor_for_mut(&mut self, idx: VirtualAddress) -> Cursor<marker::Mut<'_>, A> {
        let mut stack = ArrayVec::new();
        stack.push(self.0.borrow_mut());

        Cursor {
            stack,
            virt: idx,
            level: (A::PAGE_TABLE_LEVELS.len() - 1) as u32,
        }
    }
}

// ===== impl *Any* Cursor =====

impl<BorrowType, A: Arch> Cursor<BorrowType, A> {
    pub const fn current_level(&self) -> &'static PageTableLevel {
        &A::PAGE_TABLE_LEVELS[self.level as usize]
    }

    pub const fn current_block_size(&self) -> usize {
        A::PAGE_SIZE.pow(self.level)
    }

    pub fn current_entry(&self) -> &A::PageTableEntry {
        let index = self.current_level().table_index(self.virt);
        self.current_table().get(index)
    }

    pub fn can_insert_leaf(&self, phys: PhysicalAddress, len: usize) -> bool {
        self.virt.is_aligned_to(self.current_block_size())
            && phys.is_aligned_to(self.current_block_size())
            && len >= self.current_block_size()
            && self.current_level().supports_leaf()
    }

    pub fn descend(&mut self) -> Result<(), ()> {
        let entry = self.current_entry();

        if entry.is_vacant() || entry.is_leaf() {
            todo!() // ERROR cant descend (reason: either unmapped or leaf)
        }

        self.stack.push(Table {
            entries: entry.address(),
            _marker: PhantomData,
        });

        self.level += 1;

        Ok(())
    }

    pub fn ascend(&mut self) -> Result<Table<BorrowType, A>, ()> {
        let res = self.stack.pop().ok_or(());

        self.level -= 1;

        res
    }

    fn current_table(&self) -> &Table<BorrowType, A> {
        self.stack.last().unwrap()
    }
}

// ===== impl *Mutable* Cursor =====

impl<'a, A: Arch> Cursor<marker::Mut<'a>, A> {
    pub fn current_entry_mut(&mut self) -> &mut A::PageTableEntry {
        let index = self.current_level().table_index(self.virt);
        self.current_table_mut().get_mut(index)
    }

    pub fn insert_table(&mut self, table: Table<marker::Owned, A>) -> A::PageTableEntry {
        let idx = self.current_level().table_index(self.virt);
        self.current_table_mut().insert_table(idx, table)
    }

    pub fn insert_leaf(
        &mut self,
        block_address: PhysicalAddress,
        attributes: MemoryAttributes,
    ) -> A::PageTableEntry {
        let idx = self.current_level().table_index(self.virt);
        self.current_table_mut()
            .insert_leaf(idx, block_address, attributes)
    }

    pub fn remove_current(&mut self) -> A::PageTableEntry {
        let idx = self.current_level().table_index(self.virt);
        self.current_table_mut().remove(idx)
    }

    fn current_table_mut(&mut self) -> &mut Table<marker::Mut<'a>, A> {
        self.stack.last_mut().unwrap()
    }
}

pub(crate) mod marker {
    use core::marker::PhantomData;

    pub(crate) enum Owned {}
    pub(crate) struct Mut<'a>(PhantomData<&'a mut ()>);
    pub(crate) struct Immut<'a>(PhantomData<&'a ()>);
}
