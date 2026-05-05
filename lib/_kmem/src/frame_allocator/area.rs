// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::Range;
use core::ptr::NonNull;
use core::{cmp, fmt};

use cordyceps::List;

use crate::address_space::RawAddressSpace;
use crate::frame_allocator::AllocError;
use crate::{AddressRangeExt, Frame, PhysicalAddress};

const MAX_ORDER: usize = 11;

pub struct Area<A: RawAddressSpace> {
    area: Range<PhysicalAddress>,
    frames: &'static mut [MaybeUninit<Frame>],

    free_lists: [List<Frame>; MAX_ORDER],

    max_order: usize,
    total_frames: usize,
    used_frames: usize,

    _aspace: PhantomData<A>,
}

impl<A: RawAddressSpace> fmt::Debug for Area<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Area")
            .field("area", &self.area)
            .field(
                "frames",
                &format_args!("&[MaybeUninit<FrameInner>; {}]", self.frames.len()),
            )
            .field("free_lists", &self.free_lists)
            .field("max_order", &self.max_order)
            .field("total_frames", &self.total_frames)
            .field("used_frames", &self.used_frames)
            .finish()
    }
}

impl<A: RawAddressSpace> Area<A> {
    pub fn from_selection(
        area: Range<PhysicalAddress>,
        frames: &'static mut [MaybeUninit<Frame>],
    ) -> Self {
        let mut free_lists = [const { List::new() }; MAX_ORDER];
        let mut total_frames = 0;
        let mut max_order = 0;

        let mut remaining_bytes = area.size();
        let mut addr = area.start;

        // This is the main area initialization loop. We loop through the `area` "chopping off" the
        // largest possible min_block_size-aligned block from the area and add that to its corresponding
        // free list.
        //
        // Note: Remember that for buddy allocators `size == align`. That means we both need to check
        // the alignment and size of our remaining area and can only chop off whatever is smaller.
        while remaining_bytes > 0 {
            // println!("processing next chunk. remaining_bytes={remaining_bytes};addr={addr:?}");

            // the largest size we can chop off given the alignment of the remaining area
            let max_align = if addr == PhysicalAddress::ZERO {
                // if area happens to start exactly at address 0x0 our calculation below doesn't work.
                // address 0x0 actually supports *any* alignment so we special-case it and return `MAX`
                usize::MAX
            } else {
                // otherwise mask out the least significant bit of the address to figure out its alignment
                addr.get() & (!addr.get() + 1)
            };
            // the largest size we can chop off given the size of the remaining area
            // which is the next smaller power of two
            let max_size = 1 << remaining_bytes.ilog2();

            // our chosen size will be the smallest of
            // - the maximum size by remaining areas alignment
            // - the maximum size by remaining areas size
            // - the maximum block size supported by this allocator
            let size = cmp::min(
                cmp::min(max_align, max_size),
                A::PAGE_SIZE << (MAX_ORDER - 1),
            );
            debug_assert!(size.is_multiple_of(A::PAGE_SIZE));

            let order =
                (u8::try_from(size.trailing_zeros()).unwrap() - A::PAGE_SIZE_LOG_2) as usize;

            {
                let frame = frames[total_frames].write(Frame::from_parts(addr, 0));

                free_lists[order].push_back(NonNull::from(frame));
            }

            total_frames += 1 << order;
            max_order = cmp::max(max_order, order);
            addr = addr.checked_add(size).unwrap();
            remaining_bytes -= size;
        }

        // Make sure we've accounted for all frames
        debug_assert_eq!(total_frames, area.size() / A::PAGE_SIZE);

        Self {
            area,
            frames,

            free_lists,

            max_order,
            total_frames,
            used_frames: 0,

            _aspace: PhantomData,
        }
    }

    /// Attempts to allocate a block of frames.
    ///
    /// On success, returns a [`NonNull<[Frame]>`][NonNull] meeting the size and alignment guarantees
    /// of `layout`.
    ///
    /// The returned block may have a larger size than specified by `layout.size()`, and the frames
    /// physical contents will not be initialized.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates that either memory is exhausted or `layout` does not meet
    /// allocator's size or alignment constraints. You can check [`Self::max_alignment_hint`] for
    /// the largest alignment possibly supported by this allocator.
    pub fn allocate(&mut self, layout: Layout) -> Result<NonNull<[Frame]>, AllocError> {
        let min_order = self.allocation_order(layout)?;

        // Starting at the smallest sufficient size class, search for a free block. If we find one in
        // a free list, return it and its order.
        let (block_ptr, block_order) = self.free_lists[min_order..]
            .iter_mut()
            .enumerate()
            .find_map(|(i, list)| list.pop_back().map(|block| (block, i + min_order)))
            .ok_or(AllocError)?;

        // Safety: we pulled this pointer from the free-lists which means it must be correctly initialized
        let block = unsafe { block_ptr.as_ref() };

        // if the block we found is larger than the `min_order` we need, we repeatedly split off
        // the upper half (of decreasing size) until we reach the desired size. The split off blocks
        // are returned to their appropriate free lists.
        for order in (min_order..block_order).rev() {
            let buddy_addr = block.addr().checked_add(A::PAGE_SIZE << order).unwrap();
            let buddy = self.frame_for_addr(buddy_addr).unwrap();

            let buddy = buddy.write(Frame::from_parts(buddy_addr, 0));
            let buddy = NonNull::from(buddy);

            self.free_lists[order].push_back(buddy);
        }

        let alloc_size_frames = 1 << min_order;

        // lazily initialize all frames
        for idx in 0..alloc_size_frames {
            let addr = block.addr().checked_add(A::PAGE_SIZE * idx).unwrap();

            let frame = self.frame_for_addr(addr).unwrap();
            frame.write(Frame::from_parts(addr, 1));
        }

        self.used_frames += alloc_size_frames;

        Ok(NonNull::slice_from_raw_parts(block_ptr, alloc_size_frames))
    }

    /// Deallocates a block of frames referenced by `block`.
    ///
    /// # Safety
    ///
    /// * `block` must a block of frames managed by this area,
    /// * `block` must denote a block of frames *currently allocated* from this area, and
    /// * `layout` must *fit* that block of frames.
    pub unsafe fn deallocate(&mut self, mut block: NonNull<Frame>, layout: Layout) {
        let initial_order = self.allocation_order(layout).unwrap();
        let mut order = initial_order;

        while order < self.free_lists.len() - 1 {
            // Safety: the caller must ensure the block is currently allocated (and therefore initialized)
            let block_ = unsafe { block.as_ref() };

            if let Some(buddy) = self.buddy_addr(order, block_.addr())
                && cmp::min(block_.addr(), buddy).is_aligned_to(A::PAGE_SIZE << (order + 1))
                && self.remove_from_free_list(order, buddy)
            {
                let buddy: NonNull<Frame> =
                    NonNull::from(self.frame_for_addr(buddy).unwrap()).cast();
                block = cmp::min(buddy, block);
                order += 1;
            } else {
                break;
            }
        }

        self.free_lists[order].push_back(block);
        self.used_frames -= 1 << initial_order;
        self.max_order = cmp::max(self.max_order, order);
    }

    pub fn max_alignment_hint(&self) -> usize {
        self.order_size(self.max_order)
    }

    fn frame_for_addr(&mut self, addr: PhysicalAddress) -> Option<&mut MaybeUninit<Frame>> {
        let relative = addr.checked_sub_addr(self.area.start).unwrap();
        let idx = relative >> A::PAGE_SIZE_LOG_2;
        self.frames.get_mut(idx)
    }

    pub(crate) fn contains_frame(&self, addr: PhysicalAddress) -> bool {
        self.area.contains(&addr)
    }

    fn buddy_addr(&self, order: usize, block: PhysicalAddress) -> Option<PhysicalAddress> {
        assert!(block >= self.area.start);
        assert!(block.is_aligned_to(A::PAGE_SIZE << order));

        let relative = block.checked_sub_addr(self.area.start).unwrap();
        let size = self.order_size(order);
        if size >= self.area.size() {
            // MAX_ORDER blocks do not have buddies
            None
        } else {
            // Fun: We can find our buddy by xoring the right bit in our
            // offset from the base of the heap.
            Some(self.area.start.checked_add(relative ^ size).unwrap())
        }
    }

    fn remove_from_free_list(&mut self, order: usize, to_remove: PhysicalAddress) -> bool {
        let mut c = self.free_lists[order].cursor_front_mut();

        while let Some(candidate) = c.current() {
            if candidate.addr() == to_remove {
                c.remove_current().unwrap();
                return true;
            }

            c.move_next();
        }

        false
    }

    // The size of the blocks we allocate for a given order.
    const fn order_size(&self, order: usize) -> usize {
        1 << (A::PAGE_SIZE_LOG_2 as usize + order)
    }

    const fn allocation_size(&self, layout: Layout) -> Result<usize, AllocError> {
        // We can only allocate blocks that are at least one page
        if !layout.size().is_multiple_of(A::PAGE_SIZE) {
            return Err(AllocError);
        }

        // We can only allocate blocks that are at least page aligned
        if !layout.align().is_multiple_of(A::PAGE_SIZE) {
            return Err(AllocError);
        }

        let size = layout.size().next_power_of_two();

        // We cannot allocate blocks larger than our largest size class
        if size > self.order_size(self.free_lists.len()) {
            return Err(AllocError);
        }

        Ok(size)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "cannot use try_from in const fn"
    )]
    const fn allocation_order(&self, layout: Layout) -> Result<usize, AllocError> {
        if let Ok(size) = self.allocation_size(layout) {
            Ok((size.ilog2() as u8 - A::PAGE_SIZE_LOG_2) as usize)
        } else {
            Err(AllocError)
        }
    }

    pub fn assert_valid(&self, ctx: &str) {
        for (order, l) in self.free_lists.iter().enumerate() {
            l.assert_valid();

            for f in l {
                assert!(
                    f.addr().is_aligned_to(A::PAGE_SIZE << order),
                    "{ctx}frame {f:?} is not aligned to order {order}"
                );
            }
        }

        assert_eq!(
            frames_in_area(self) + self.used_frames,
            self.total_frames,
            "{ctx}actual number of frames does not match counters"
        );
    }
}

fn frames_in_area<A: RawAddressSpace>(area: &Area<A>) -> usize {
    let mut frames = 0;
    for (order, l) in area.free_lists.iter().enumerate() {
        frames += l.len() << order;
    }
    frames
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use proptest::{prop_assert, prop_assert_eq, prop_assume, prop_compose, proptest};

    use super::*;
    use crate::test_utils::TestAddressSpace;

    const PAGE_SIZE: usize = 4096;

    prop_compose! {
        // Generate arbitrary integers up to half the maximum desired value,
        // then multiply them by 2, thus producing only even integers in the
        // desired range.
        fn page_aligned(max: usize)(base in 0..max/PAGE_SIZE) -> usize { base * PAGE_SIZE }
    }

    proptest! {
        #[test]
        fn new_fixed_base(num_frames in 0..50_000usize) {
            let mut area: Area<TestAddressSpace<PAGE_SIZE, 38>> = Area::from_selection(
                PhysicalAddress::ZERO..PhysicalAddress::new(num_frames * PAGE_SIZE),
                {
                    let mut frames: Vec<MaybeUninit<Frame>> = Vec::with_capacity(num_frames);
                    frames.resize_with(num_frames, || MaybeUninit::uninit());
                    frames.leak()
                }
            );
            area.assert_valid("");

            // let's check whether the area correctly initialized itself
            //
            // since we start on an aligned base address (0x0) we expect it have split off chunks
            // largest-to-smallest. We replicate the process here, but take a block from its free list.
            let mut frames_remaining = num_frames;
            while frames_remaining > 0 {
                // clamp the order we calculate at the max possible order
                let chunk_order = cmp::min(frames_remaining.ilog2() as usize, MAX_ORDER - 1);

                let chunk = area.free_lists[chunk_order].pop_back();
                prop_assert!(chunk.is_some(), "expected chunk of order {chunk_order}");

                frames_remaining -= 1 << chunk_order;
            }
            // At the end of this process we expect all free lists to be empty
            prop_assert!(area.free_lists.iter().all(|list| list.is_empty()));
        }

        #[test]
        fn new_arbitrary_base(num_frames in 0..50_000usize, area_start in page_aligned(usize::MAX)) {

            let area = {
                let area_end = area_start.checked_add(num_frames * PAGE_SIZE);
                prop_assume!(area_end.is_some());
                PhysicalAddress::new(area_start)..PhysicalAddress::new(area_end.unwrap())
            };

            let area: Area<TestAddressSpace<PAGE_SIZE, 38>> = Area::from_selection(
                area,
                {
                    let mut frames: Vec<MaybeUninit<Frame>> = Vec::with_capacity(num_frames);
                    frames.resize_with(num_frames, || MaybeUninit::uninit());
                    frames.leak()
                }
            );
            area.assert_valid("");

            // TODO figure out if we can test the free lists in a sensible way
        }

        #[test]
        fn alloc_exhaustion(num_frames in 0..5_000usize, area_start in page_aligned(usize::MAX)) {
            let area = {
                let area_end = area_start.checked_add(num_frames * PAGE_SIZE);
                prop_assume!(area_end.is_some());
                PhysicalAddress::new(area_start)..PhysicalAddress::new(area_end.unwrap())
            };

            let mut area: Area<TestAddressSpace<PAGE_SIZE, 38>> = Area::from_selection(
                area,
                {
                    let mut frames: Vec<MaybeUninit<Frame>> = Vec::with_capacity(num_frames);
                    frames.resize_with(num_frames, || MaybeUninit::uninit());
                    frames.leak()
                }
            );
            area.assert_valid("");

            debug_assert_eq!(frames_in_area(&mut area), num_frames);
        }

        #[test]
        fn alloc_dealloc(num_frames in 0..5_000usize, area_start in page_aligned(usize::MAX), alloc_frames in 1..500usize) {
            let area = {
                let area_end = area_start.checked_add(num_frames * PAGE_SIZE);
                prop_assume!(area_end.is_some());
                PhysicalAddress::new(area_start)..PhysicalAddress::new(area_end.unwrap())
            };

            let area1: Area<TestAddressSpace<PAGE_SIZE, 38>> = Area::from_selection(
                area.clone(),
               {
                    let mut frames: Vec<MaybeUninit<Frame>> = Vec::with_capacity(num_frames);
                    frames.resize_with(num_frames, || MaybeUninit::uninit());
                    frames.leak()
                }
            );
            area1.assert_valid("");

            let mut area2: Area<TestAddressSpace<PAGE_SIZE, 38>> = Area::from_selection(
                area,
                {
                    let mut frames: Vec<MaybeUninit<Frame>> = Vec::with_capacity(num_frames);
                    frames.resize_with(num_frames, || MaybeUninit::uninit());
                    frames.leak()
                }
            );
            area2.assert_valid("");

            // we can only allocate contiguous blocks of the largest order available
            prop_assume!(alloc_frames < (area2.max_alignment_hint() / PAGE_SIZE));

            let layout = Layout::from_size_align(alloc_frames * PAGE_SIZE, PAGE_SIZE).unwrap();

            let block = area2.allocate(layout).unwrap();
            prop_assert!(block.len() >= alloc_frames);

            unsafe { area2.deallocate(block.cast(), layout); }

            assert_eq!(frames_in_area(&area2), num_frames);

            for (order, (f1, f2)) in area1.free_lists.iter().zip(area2.free_lists.iter()).enumerate() {
                prop_assert_eq!(f1.len(), f2.len(), "free lists at order {} have different lengths {} vs {}", order, f1.len(), f2.len());
            }
        }
    }
}
