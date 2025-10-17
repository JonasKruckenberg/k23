mod table;

use core::num::NonZeroUsize;

use table::{marker, Root, Table};

use crate::arch::{Arch, PageTableEntry as _};
use crate::hw_aspace::table::Cursor;
use crate::{AllocError, FrameAllocator, MemoryAttributes, PhysicalAddress, VirtualAddress};

pub struct HardwareAddressSpace<'alloc, A: Arch> {
    arch: A,
    root: Root<A>,
    frame_allocator: &'alloc dyn FrameAllocator<A>,
}

impl<'alloc, A: Arch> HardwareAddressSpace<'alloc, A> {
    pub fn new(
        arch: A,
        frame_allocator: &'alloc dyn FrameAllocator<A>,
    ) -> Result<Self, AllocError> {
        let root = Root::from_owned(Table::allocate(frame_allocator)?);

        Ok(Self {
            arch,
            root,
            frame_allocator,
        })
    }

    pub fn lookup(&self, virt: VirtualAddress) -> Option<(PhysicalAddress, MemoryAttributes)> {
        // cursor pointing to the virtual address
        let mut c = self.root.cursor_for(virt);

        // NB: iterate over the levels to have an explicit upper bound on the loop
        for _ in A::PAGE_TABLE_LEVELS {
            let entry = c.current_entry();

            if entry.is_vacant() {
                return None;
            } else if entry.is_leaf() {
                return Some((entry.address(), entry.attributes()));
            } else {
                c.descend().unwrap();
            }
        }

        None
    }

    pub unsafe fn map(
        &mut self,
        virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        attributes: MemoryAttributes,
    ) {
        self.for_range_mut(virt, len, |mut c, remaining_bytes| {
            // NB: iterate over the levels to have an explicit upper bound on the loop
            for _ in A::PAGE_TABLE_LEVELS {
                let entry = c.current_entry();

                if entry.is_leaf() {
                    todo!() // ERROR already mapped
                }

                if entry.is_vacant() && c.can_insert_leaf(phys, remaining_bytes) {
                    // We can map at this level => insert a leaf entry and continue
                    c.insert_leaf(phys, attributes);

                    let block_size = c.current_block_size();
                    phys = phys.add(block_size);
                    return Ok(block_size);
                }

                // The entry is a *Table* OR a *Vacant* entry we cannot map into (for whatever reason)
                // - If it's a *Table* we will successfully descend here,
                // - if it's a *Vacant* one we will have to allocate a new table and retry
                if c.descend().is_err() {
                    let table = Table::allocate(self.frame_allocator).unwrap(); // TODO error

                    c.insert_table(table);

                    // Retry descending, this time this must not fail, we allocated and inserted the table above.
                    c.descend().unwrap();
                }
            }

            unreachable!() // couldn't map
        })
        .unwrap();

        // TODO memory barrier
    }

    pub unsafe fn remap(
        &mut self,
        virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
    ) {
        self.for_range_mut(virt, len, |mut c, remaining_bytes| {
            // NB: iterate over the levels to have an explicit upper bound on the loop
            for _ in A::PAGE_TABLE_LEVELS {
                let entry = c.current_entry();

                if entry.is_vacant() {
                    todo!() // ERROR not mapped
                }

                if entry.is_leaf() && c.can_insert_leaf(phys, remaining_bytes) {
                    // We can map at this level => insert a leaf entry and continue

                    unsafe { c.current_entry_mut().set_address(phys) };

                    let block_size = c.current_block_size();
                    phys = phys.add(block_size);
                    return Ok(block_size);
                }

                // The entry is a *Table* OR a *Leaf* entry we cannot override (for whatever reason)
                // - If it's a *Table* we will successfully descend here,
                // - if it's a *Leaf* we will have to allocate a new table, override the current entry
                //   and retry
                if c.descend().is_err() {
                    let table = Table::allocate(self.frame_allocator).unwrap(); // TODO error

                    c.insert_table(table);

                    // Retry descending, this time this must not fail, we allocated and inserted the table above.
                    c.descend().unwrap();
                }
            }

            unreachable!() // couldn't map
        })
        .unwrap();

        // TODO memory barrier
    }

    pub unsafe fn set_attributes(
        &mut self,
        virt: VirtualAddress,
        len: NonZeroUsize,
        attributes: MemoryAttributes,
    ) {
        self.for_range_mut(virt, len, |mut c, _| {
            // NB: iterate over the levels to have an explicit upper bound on the loop
            for _ in A::PAGE_TABLE_LEVELS {
                let entry = c.current_entry();

                if entry.is_vacant() {
                    todo!() // ERROR not mapped
                }

                if entry.is_leaf() {
                    unsafe { c.current_entry_mut().set_attributes(attributes) };

                    let block_size = c.current_block_size();
                    return Ok(block_size);
                }

                c.descend().unwrap();
            }

            unreachable!() // couldn't map
        })
        .unwrap();

        // TODO memory barrier
    }

    pub unsafe fn unmap(&mut self, mut virt: VirtualAddress, len: NonZeroUsize) {
        self.for_range_mut(virt, len, |mut c, _| {
            // FIRST PHASE: clear the actual leaf page
            let bytes_processed = loop {
                if c.current_entry().is_leaf() {
                    c.remove_current();

                    let block_size = c.current_block_size();
                    virt = virt.add(block_size);
                    break block_size;
                }

                c.descend().unwrap();
            };

            // SECOND PHASE: ascend up the tree and deallocate any fully empty page tables
            // NB: iterate over the levels to have an explicit upper bound on the loop
            for _ in A::PAGE_TABLE_LEVELS {
                let Ok(table) = c.ascend() else {
                    // we cannot ascend anymore, we're done
                    break;
                };

                if table.is_empty(c.current_level()) {
                    let entry = c.remove_current();
                    debug_assert!(!entry.is_vacant() && !entry.is_leaf());

                    let table_owned = unsafe { table.cast_owned() };

                    unsafe { table_owned.deallocate(self.frame_allocator) };
                }
            }

            Ok(bytes_processed)
        })
        .unwrap();

        // TODO memory barrier
    }

    pub unsafe fn activate(&mut self) {
        unsafe { self.arch.set_active_table(self.root.address()) }
    }

    fn for_range_mut<F>(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        mut cb: F,
    ) -> Result<(), ()>
    where
        F: FnMut(Cursor<marker::Mut<'_>, A>, usize) -> Result<usize, ()>,
    {
        let mut remaining_bytes = len.get();
        while remaining_bytes > 0 {
            // cursor pointing to the virtual address
            let c = self.root.cursor_for_mut(virt);

            let bytes_processed = cb(c, remaining_bytes)?;

            virt = virt.add(bytes_processed);
            remaining_bytes -= bytes_processed;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // TODO test cases for hardware address space
    // - lookup
    // - map
    // - remap
    // - set attributes
    // - unmap
}