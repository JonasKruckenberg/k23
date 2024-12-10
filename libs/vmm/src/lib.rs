#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::boxed::Box;
use core::alloc::Layout;
use core::fmt::Formatter;
use core::mem::offset_of;
use core::num::{NonZero, NonZeroUsize};
use core::ops::Range;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{cmp, fmt};
use pin_project_lite::pin_project;
use pmm::frame_alloc::{BitMapAllocator, BumpAllocator};
use pmm::{Flush, VirtualAddress};
use wavltree::Entry;

pub struct PageAllocator {}
impl PageAllocator {
    pub fn allocate(&mut self, layout: Layout, flags: pmm::Flags) {}
}

pub struct AddressSpace<A> {
    tree: wavltree::WAVLTree<Mapping>,
    arch: pmm::AddressSpace<A>,
    frame_alloc: BitMapAllocator,
}
impl<A> AddressSpace<A>
where
    A: pmm::arch::Arch,
{
    pub fn new(
        arch: pmm::AddressSpace<A>,
        bump_allocator: BumpAllocator,
        phys_offset: VirtualAddress,
    ) -> Self {
        Self {
            tree: wavltree::WAVLTree::default(),
            arch,
            frame_alloc: BitMapAllocator::new(bump_allocator, phys_offset).unwrap(),
        }
    }

    /// Map an object into virtual memory
    pub fn map(
        &mut self,
        range: Range<VirtualAddress>,
        flags: pmm::Flags,
        vmo: (),
        vmo_offset: (),
    ) {
        todo!()
    }

    pub fn reserve(&mut self, range: Range<VirtualAddress>, flags: pmm::Flags) {
        // FIXME turn these into errors instead of panics
        match self.tree.entry(&range.start) {
            Entry::Occupied(_) => panic!("already reserved"),
            Entry::Vacant(mut entry) => {
                if let Some(next_mapping) = entry.peek_next_mut() {
                    assert!(range.end <= next_mapping.range.start);

                    if next_mapping.range.start == range.end && next_mapping.flags == flags {
                        next_mapping.project().range.start = range.start;
                        return;
                    }
                }

                if let Some(prev_mapping) = entry.peek_prev_mut() {
                    assert!(prev_mapping.range.end <= range.start);

                    if prev_mapping.range.end == range.start && prev_mapping.flags == flags {
                        prev_mapping.project().range.end = range.end;
                        return;
                    }
                }

                entry.insert(Box::pin(Mapping::new(range, flags)));
            }
        }
    }

    pub fn unmap(&mut self, range: Range<VirtualAddress>) {
        let mut iter = self.tree.range_mut(range.clone());
        let mut flush = Flush::empty(self.arch.asid());

        while let Some(mapping) = iter.next() {
            log::trace!("{mapping:?}");
            let base = cmp::max(mapping.range.start, range.start);
            let len = cmp::min(mapping.range.end, range.end).sub_addr(base);

            if range.start <= mapping.range.start && range.end >= mapping.range.end {
                // this mappings range is entirely contained within `range`, so we need
                // fully remove the mapping from the tree
                // TODO verify if this absolutely insane code is actually any good

                let ptr = NonNull::from(mapping.get_mut());
                let mut cursor = unsafe { iter.tree().cursor_mut_from_ptr(ptr) };
                let mapping = cursor.remove().unwrap();

                self.arch
                    .unmap(
                        &mut self.frame_alloc,
                        mapping.range.start,
                        NonZero::new(mapping.range.end.sub_addr(mapping.range.start)).unwrap(),
                        &mut flush,
                    )
                    .unwrap();
            } else if range.start > mapping.range.start && range.end < mapping.range.end {
                // `range` is entirely contained within the mappings range, we
                // need to split the range in two

                let mapping = mapping.project();
                let left = mapping.range.start..range.start;

                mapping.range.start = range.end;
                iter.tree()
                    .insert(Box::pin(Mapping::new(left, mapping.flags.clone())));
            } else if range.start > mapping.range.start {
                // `range` is mostly past this mappings range, but overlaps partially
                // we need adjust the ranges end

                let mapping = mapping.project();
                mapping.range.end = range.start;
            } else if range.end < mapping.range.end {
                // `range` is mostly before this mappings range, but overlaps partially
                // we need adjust the ranges start

                let mapping = mapping.project();
                mapping.range.start = range.end;
            } else {
                unreachable!()
            }

            log::trace!("decommit {base:?}..{:?}", base.add(len));
            self.arch
                .unmap(
                    &mut self.frame_alloc,
                    base,
                    NonZeroUsize::new(len).unwrap(),
                    &mut flush,
                )
                .unwrap();
        }

        flush.flush().unwrap();
    }

    // behaviour:
    //  - `range` must be fully mapped
    //  - `new_flags` must be a subset of the current mappings flags (permissions can only be reduced)
    //  - `range` must not be empty
    //  - the above checks are done atomically ie they hold for all affected mappings
    //  - if old and new flags are the same protect is a no-op
    pub fn protect(&mut self, range: Range<VirtualAddress>, new_flags: pmm::Flags) {
        let iter = self.tree.range(range.clone());

        assert!(!range.is_empty());

        // check whether part of the range is not mapped, or the new flags are invalid for some mapping
        // in the range. If so, we need to terminate before actually materializing any changes
        let mut bytes_checked = 0;
        for mapping in iter {
            assert!(mapping.flags.contains(new_flags));
            bytes_checked += mapping.range.end.sub_addr(mapping.range.start);
        }
        assert_eq!(bytes_checked, range.end.sub_addr(range.start));

        // at this point we know the operation is valid, so can start updating the mappings
        let mut iter = self.tree.range_mut(range.clone());
        let mut flush = Flush::empty(self.arch.asid());

        while let Some(mapping) = iter.next() {
            // If the old and new flags are the same, nothing need to be materialized
            if mapping.flags == new_flags {
                continue;
            }

            if new_flags.is_empty() {
                let ptr = NonNull::from(mapping.get_mut());
                let mut cursor = unsafe { iter.tree().cursor_mut_from_ptr(ptr) };
                let mapping = cursor.remove().unwrap();

                self.arch
                    .unmap(
                        &mut self.frame_alloc,
                        mapping.range.start,
                        NonZero::new(mapping.range.end.sub_addr(mapping.range.start)).unwrap(),
                        &mut flush,
                    )
                    .unwrap();
            } else {
                let base = cmp::max(mapping.range.start, range.start);
                let len = NonZeroUsize::new(cmp::min(mapping.range.end, range.end).sub_addr(base))
                    .unwrap();

                self.arch.protect(base, len, new_flags, &mut flush).unwrap();
                *mapping.project().flags = new_flags;
            }
        }

        // synchronize the changes
        flush.flush().unwrap();
    }

    pub fn commit(&mut self, range: Range<VirtualAddress>) {
        todo!()
    }

    pub fn decommit(&mut self, range: Range<VirtualAddress>) {
        todo!()
    }
}

pin_project! {
    pub struct Mapping {
        range: Range<VirtualAddress>,
        flags: pmm::Flags,
        links: wavltree::Links<Mapping>,
    }
}
impl fmt::Debug for Mapping {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mapping")
            .field("range", &self.range)
            .field("flags", &self.flags)
            .finish_non_exhaustive()
    }
}
impl Mapping {
    pub fn new(range: Range<VirtualAddress>, flags: pmm::Flags) -> Self {
        Self {
            links: wavltree::Links::default(),
            range,
            flags,
        }
    }
}

unsafe impl wavltree::Linked for Mapping {
    /// Any heap-allocated type that owns an element may be used.
    ///
    /// An element *must not* move while part of an intrusive data
    /// structure. In many cases, `Pin` may be used to enforce this.
    type Handle = Pin<Box<Self>>;

    type Key = VirtualAddress;

    /// Convert an owned `Handle` into a raw pointer
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }

    fn get_key(&self) -> &Self::Key {
        &self.range.start
    }
}

