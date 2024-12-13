#![allow(unused)]

use alloc::boxed::Box;
use alloc::vec;
use core::cmp::Ordering;
use core::fmt::Formatter;
use core::mem::offset_of;
use core::num::{NonZero, NonZeroUsize};
use core::ops::Range;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{cmp, fmt};
use loader_api::BootInfo;
use pin_project_lite::pin_project;
use pmm::frame_alloc::{BitMapAllocator, BumpAllocator, FrameUsage};
use pmm::{Flush, PhysicalAddress, VirtualAddress};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use sync::{Mutex, OnceLock};
use wavltree::Entry;
use crate::machine_info::MachineInfo;

pub static KERNEL_ASPACE: OnceLock<Mutex<AddressSpace>> = OnceLock::new();

const KERNEL_ASID: usize = 0;

pub fn init(boot_info: &BootInfo, minfo: &MachineInfo) {
    KERNEL_ASPACE.get_or_init(|| {
        let mut memories = vec![];
        for i in 0..boot_info.memory_regions_len {
            let region = unsafe { boot_info.memory_regions.add(i).as_ref().unwrap() };
            if region.kind.is_usable() {
                memories.push(region.range.clone());
            }
        }
        memories.sort_unstable_by(compare_memory_regions);

        let bump_alloc = BumpAllocator::new(&memories);
        let (arch, mut flush) =
            pmm::AddressSpace::from_active(KERNEL_ASID, boot_info.physical_memory_offset);

        let prng = ChaCha20Rng::from_seed(minfo.rng_seed.unwrap()[0..32].try_into().unwrap());
        let mut aspace = AddressSpace::new(arch, bump_alloc, prng);

        log::debug!("unmapping loader {:?}...", boot_info.loader_region);
        let loader_region_len = boot_info
            .loader_region
            .end
            .sub_addr(boot_info.loader_region.start);
        aspace
            .arch
            .unmap(
                &mut IgnoreAlloc,
                boot_info.loader_region.start,
                NonZeroUsize::new(loader_region_len).unwrap(),
                &mut flush,
            )
            .unwrap();
        flush.flush().unwrap();

        Mutex::new(aspace)
    });
}

fn compare_memory_regions(a: &Range<PhysicalAddress>, b: &Range<PhysicalAddress>) -> Ordering {
    if a.end <= b.start {
        Ordering::Less
    } else if b.end <= a.start {
        Ordering::Greater
    } else {
        // This should never happen if the `exclude_region` code about is correct
        unreachable!("Memory region {a:?} and {b:?} are overlapping");
    }
}

struct IgnoreAlloc;
impl pmm::frame_alloc::FrameAllocator for IgnoreAlloc {
    fn allocate_contiguous(
        &mut self,
        frames: NonZeroUsize,
    ) -> Result<(PhysicalAddress, NonZeroUsize), pmm::Error> {
        unimplemented!()
    }

    fn deallocate(
        &mut self,
        _base: PhysicalAddress,
        _frames: NonZeroUsize,
    ) -> Result<(), pmm::Error> {
        Ok(())
    }

    fn frame_usage(&self) -> FrameUsage {
        unreachable!()
    }
}

pub struct AddressSpace {
    tree: wavltree::WAVLTree<Mapping>,
    frame_alloc: BitMapAllocator,
    arch: pmm::AddressSpace,
    prng: Option<ChaCha20Rng>,
}
impl AddressSpace {
    pub fn new(arch: pmm::AddressSpace, bump_allocator: BumpAllocator, prng: ChaCha20Rng) -> Self {
        Self {
            tree: wavltree::WAVLTree::default(),
            frame_alloc: BitMapAllocator::new(bump_allocator, arch.physical_memory_offset())
                .unwrap(),
            arch,
            prng: Some(prng),
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

    // pub fn reserve(&mut self, range: Range<VirtualAddress>, flags: pmm::Flags) {
    //     // FIXME turn these into errors instead of panics
    //     match self.tree.entry(&range.start) {
    //         Entry::Occupied(_) => panic!("already reserved"),
    //         Entry::Vacant(mut entry) => {
    //             if let Some(next_mapping) = entry.peek_next_mut() {
    //                 assert!(range.end <= next_mapping.range.start);
    //
    //                 if next_mapping.range.start == range.end && next_mapping.flags == flags {
    //                     next_mapping.project().range.start = range.start;
    //                     return;
    //                 }
    //             }
    //
    //             if let Some(prev_mapping) = entry.peek_prev_mut() {
    //                 assert!(prev_mapping.range.end <= range.start);
    //
    //                 if prev_mapping.range.end == range.start && prev_mapping.flags == flags {
    //                     prev_mapping.project().range.end = range.end;
    //                     return;
    //                 }
    //             }
    //
    //             entry.insert(Box::pin(Mapping::new(range, flags)));
    //         }
    //     }
    // }

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
                    .insert(Box::pin(Mapping::new(left, *mapping.flags)));
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
        links: wavltree::Links<Mapping>,
        range: Range<VirtualAddress>,
        flags: pmm::Flags,
        min_first_byte: VirtualAddress,
        max_last_byte: VirtualAddress,
        max_gap: usize
    }
}
impl fmt::Debug for Mapping {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mapping")
            .field("range", &self.range)
            .field("flags", &self.flags)
            .field("min_first_byte", &self.min_first_byte)
            .field("max_last_byte", &self.max_last_byte)
            .field("max_gap", &self.max_gap)
            .finish_non_exhaustive()
    }
}
impl Mapping {
    pub fn new(range: Range<VirtualAddress>, flags: pmm::Flags) -> Self {
        Self {
            links: wavltree::Links::default(),
            min_first_byte: range.start,
            max_last_byte: range.end,
            range,
            flags,
            max_gap: 0,
        }
    }

    unsafe fn update(
        mut node: NonNull<Self>,
        left: Option<NonNull<Self>>,
        right: Option<NonNull<Self>>,
    ) {
        let node = node.as_mut();
        let mut left_max_gap = 0;
        let mut right_max_gap = 0;

        if let Some(left) = left {
            let left = left.as_ref();
            let left_gap = gap(left.max_last_byte, node.range.start);
            left_max_gap = cmp::max(left_gap, left.max_gap);
            node.min_first_byte = left.min_first_byte;
        } else {
            node.min_first_byte = node.range.start;
        }

        if let Some(right) = right {
            let right = right.as_ref();
            let right_gap = gap(node.range.end, right.min_first_byte);
            right_max_gap = cmp::max(right_gap, unsafe { right.max_gap });
            node.max_last_byte = right.max_last_byte;
        } else {
            node.max_last_byte = node.range.end;
        }

        node.max_gap = cmp::max(left_max_gap, right_max_gap);

        fn gap(left_last_byte: VirtualAddress, right_first_byte: VirtualAddress) -> usize {
            debug_assert!(
                left_last_byte < right_first_byte,
                "subtraction would underflow: {left_last_byte} >= {right_first_byte}"
            );
            right_first_byte.sub_addr(left_last_byte)
        }
    }

    fn propagate_to_root(mut maybe_node: Option<NonNull<Self>>) {
        while let Some(node) = maybe_node {
            let links = unsafe { &node.as_ref().links };
            unsafe {
                Self::update(node, links.left(), links.right());
            }
            maybe_node = links.parent();
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

    fn after_insert(self: Pin<&mut Self>) {
        debug_assert_eq!(self.min_first_byte, self.range.start);
        debug_assert_eq!(self.max_last_byte, self.range.end);
        debug_assert_eq!(self.max_gap, 0);
        Self::propagate_to_root(self.links.parent());
    }

    fn after_remove(self: Pin<&mut Self>, parent: Option<NonNull<Self>>) {
        Self::propagate_to_root(parent);
    }

    fn after_rotate(
        mut self: Pin<&mut Self>,
        parent: NonNull<Self>,
        sibling: Option<NonNull<Self>>,
        lr_child: Option<NonNull<Self>>,
        side: Side,
    ) {
        log::trace!("after rotate pivot: {self:?} parent: {parent:?} sibling: {sibling:?} lr_child: {lr_child:?}");

        let mut this = self.project();
        let _parent = unsafe { parent.as_ref() };

        *this.min_first_byte = _parent.min_first_byte;
        *this.max_last_byte = _parent.max_last_byte;
        *this.max_gap = _parent.max_gap;

        if side == Side::Left {
            unsafe {
                Self::update(parent, sibling, lr_child);
            }
        } else {
            unsafe {
                Self::update(parent, lr_child, sibling);
            }
        }
    }
}
