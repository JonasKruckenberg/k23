// // Copyright 2025. Jonas Kruckenberg
// //
// // Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// // http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// // http://opensource.org/licenses/MIT>, at your option. This file may not be
// // copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::mem::{MaybeUninit, offset_of};
use core::num::NonZeroUsize;
use core::ops::Range;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{cmp, fmt, ptr};

use cordyceps::{Linked, List, list};
use pin_project::pin_project;

use crate::PhysicalAddress;
use crate::addresses::AddressRangeExt;

#[derive(Debug)]
pub struct AllocError;

const DEFAULT_MAX_ORDER: usize = 11;

struct Area<const MAX_ORDER: usize = DEFAULT_MAX_ORDER> {
    area: Range<PhysicalAddress>,
    bookkeeping: NonNull<[MaybeUninit<Frame>]>,
    frame_size: usize,
    frame_size_log2: u8,

    free_lists: [List<Frame>; MAX_ORDER],
    max_order: usize,

    total_frames: usize,
    used_frames: usize,
}

struct FrameRef(NonNull<Frame>);

/// Metadata describing a single frame of physical memory.
#[pin_project(!Unpin)]
#[derive(Debug)]
struct Frame {
    address: PhysicalAddress,
    refcount: AtomicUsize,

    #[pin]
    next: list::Links<Self>,
}

// ===== impl Area =====

impl fmt::Debug for Area {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Area")
            .field("area", &self.area)
            .field(
                "bookkeeping",
                &format_args!("&[MaybeUninit<FrameInner>; {}]", self.bookkeeping.len()),
            )
            .field("max_order", &self.max_order)
            .field("total_frames", &self.total_frames)
            .field("used_frames", &self.used_frames)
            .field_with("free_lists", |f| {
                let mut f = f.debug_map();
                for (order, l) in self.free_lists.iter().enumerate() {
                    f.key(&order);
                    f.value_with(|f| f.debug_list().entries(l.iter()).finish());
                }
                f.finish()
            })
            .finish()
    }
}

impl<const MAX_ORDER: usize> Area<MAX_ORDER> {
    pub fn from_parts(
        area: Range<PhysicalAddress>,
        bookkeeping: NonNull<[MaybeUninit<Frame>]>,
        frame_size: usize,
    ) -> Self {
        let mut remaining_bytes = area.size();
        let mut addr = area.start;

        let mut total_frames = 0;
        let mut max_order = 0;
        let frame_size_log2 = ilog2(frame_size);
        let mut free_lists = [const { List::new() }; MAX_ORDER];

        // This is the main area initialization loop. We loop through the `area` "chopping off" the
        // largest possible min_block_size-aligned block from the area and add that to its corresponding
        // free list.
        //
        // Note: Remember that for buddy allocators `size == align`. That means we both need to check
        // the alignment and size of our remaining area and can only chop off whatever is smaller.
        while remaining_bytes > 0 {
            println!("processing next chunk. remaining_bytes={remaining_bytes};addr={addr:?}");

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
            let max_size = prev_power_of_two(remaining_bytes);

            // our chosen size will be the smallest of
            // - the maximum size by remaining areas alignment
            // - the maximum size by remaining areas size
            // - the maximum block size supported by this allocator
            let size = cmp::min(cmp::min(max_align, max_size), frame_size << (MAX_ORDER - 1));
            debug_assert!(size.is_multiple_of(frame_size));

            let order = (ilog2(size) - frame_size_log2) as usize;

            unsafe {
                let frame = bookkeeping.cast::<MaybeUninit<Frame>>().add(total_frames);

                frame.write(MaybeUninit::new(Frame::new(addr, 0)));

                free_lists[order].push_back(frame.cast());
            }

            println!(
                "processed chunk: {} frames, addr {addr:?} (remaining {remaining_bytes})",
                1 << order
            );
            total_frames += 1 << order;
            max_order = cmp::max(max_order, order);
            addr = addr.checked_add(size).unwrap();
            remaining_bytes -= size;
        }

        // Make sure we've accounted for all frames
        debug_assert_eq!(total_frames, area.size() / frame_size);

        Self {
            area,
            bookkeeping,
            frame_size,
            frame_size_log2,
            free_lists,
            max_order,
            total_frames,
            used_frames: 0,
        }
    }

    pub fn is_exhausted(&self) -> bool {
        self.used_frames == self.total_frames
    }

    pub fn allocate(&mut self, layout: Layout, out: &mut List<Frame>) -> Result<(), AllocError> {
        let min_order = self.allocation_order(layout)?;

        // Search through our free lists, starting with the smallest acceptable size class, for a free
        // block.
        let (block, block_order) = self.free_lists[min_order..=self.max_order]
            .iter_mut()
            .enumerate()
            .find_map(|(idx, list)| list.pop_back().map(|block| (block, idx + min_order)))
            .ok_or(AllocError)?;

        println!("found block of size {block_order}");

        // If the free block we found was of a larger size class than we need, we split
        // it in half retuning the split off half to the free lists. We continue this process downwards
        // until we hit our desired size class.
        for order in (min_order..=block_order).rev() {
            let buddy: NonNull<MaybeUninit<Frame>> = unsafe { block.add(1 << order).cast() };

            // initialize the buddy frame with a refcount of 0: it is still free and belongs to us.
            self.initialize_frame(buddy, 0);

            self.free_lists[order].push_back(buddy.cast())
        }

        // Iterate through all frames in the block, initialize them and add them to the free list
        for frame_idx in (0..(1 << min_order)).rev() {
            let frame: NonNull<MaybeUninit<Frame>> = unsafe { block.add(frame_idx).cast() };

            // initialize all frames to a refcount of 1: we have just allocated them.
            self.initialize_frame(frame, 1);

            // return the frame!
            out.push_back(frame.cast());
        }

        self.used_frames += 1 << min_order;

        Ok(())
    }

    pub unsafe fn deallocate(&mut self, mut block: NonNull<Frame>, layout: Layout) {
        let initial_order = self.allocation_order(layout).unwrap();

        for order in initial_order..MAX_ORDER {
            let buddy =
                block.map_addr(|addr| NonZeroUsize::new(addr.get() ^ (1 << order)).unwrap());

            unsafe {
                println!(
                    "trying to merge block {:?} with buddy {:?} at order {order}",
                    block.as_ref(),
                    buddy.as_ref()
                );
            }

            // is our buddy block free as well?
            if self.remove_from_free_list(order, buddy) {
                // then merge the two blocks!
                block = cmp::min(block, buddy);
                continue;
            }

            self.free_lists[order].push_back(block);
            self.used_frames -= 1 << initial_order;
            return;
        }

        // println!("deallocating block {block:?} {layout:?} {initial_order:?}");
        //
        //
        //
        // }
    }

    fn initialize_frame(
        &mut self,
        mut frame: NonNull<MaybeUninit<Frame>>,
        initial_refcount: usize,
    ) {
        let frame_idx =
            unsafe { frame.offset_from_unsigned(self.bookkeeping.cast::<MaybeUninit<Frame>>()) };

        let frame_addr = self
            .area
            .start
            .checked_add(frame_idx * self.frame_size)
            .unwrap();

        let frame = unsafe { frame.as_mut() };
        frame.write(Frame::new(frame_addr, initial_refcount));
    }

    fn remove_from_free_list(&mut self, order: usize, expected: NonNull<Frame>) -> bool {
        for found in self.free_lists[order].iter_raw() {
            if ptr::addr_eq(found.as_ptr(), expected.as_ptr()) {
                assert!(unsafe { self.free_lists[order].remove(found).is_some() });
                return true;
            }
        }

        false
    }

    /// The size of the blocks we allocate for a given order.
    const fn order_size(&self, order: usize) -> usize {
        1 << (self.frame_size_log2 as usize + order)
    }

    const fn allocation_size(&self, layout: Layout) -> Result<usize, AllocError> {
        // We can only allocate blocks that are at least page aligned
        if !layout.align().is_multiple_of(self.frame_size) {
            return Err(AllocError);
        }

        let size = layout.pad_to_align().size();

        // We cannot allocate blocks larger than our largest size class
        if size > self.order_size(self.max_order) {
            return Err(AllocError);
        }

        Ok(size)
    }

    const fn allocation_order(&self, layout: Layout) -> Result<usize, AllocError> {
        if let Ok(size) = self.allocation_size(layout) {
            Ok((ilog2(size) - self.frame_size_log2) as usize)
        } else {
            Err(AllocError)
        }
    }
}

const fn ilog2(n: usize) -> u8 {
    let mut temp = n;
    let mut result = 0;
    temp >>= 1;
    while temp != 0 {
        result += 1;
        temp >>= 1;
    }
    result
}

const fn prev_power_of_two(n: usize) -> usize {
    1 << ilog2(n)
}

// ===== impl Frame =====

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
            && self.refcount() == other.refcount()
            && self.next.is_linked() == other.next.is_linked()
    }
}

impl Frame {
    const fn new(address: PhysicalAddress, initial_refcount: usize) -> Self {
        Self {
            address,
            refcount: AtomicUsize::new(initial_refcount),
            next: list::Links::new(),
        }
    }

    pub fn refcount(&self) -> usize {
        self.refcount.load(Ordering::Acquire)
    }

    pub fn is_unique(&self) -> bool {
        self.refcount() >= 1
    }

    pub fn is_free(&self) -> bool {
        self.refcount() == 0
    }
}

unsafe impl Linked<list::Links<Self>> for Frame {
    type Handle = NonNull<Self>;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<list::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, next);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use proptest::{prop_assert, prop_assert_eq, proptest};

    use super::*;

    proptest! {
        // Assert that freelists are set up correctly (big chunks are prioritized) for any number of
        // frames
        #[test]
        fn creation_fixed_start(frames in 0usize..50_000usize) {
            const PAGE_SIZE: usize = 4096;

            let mut bookkeeping: Vec<MaybeUninit<Frame>> = Vec::with_capacity(frames);
            bookkeeping.resize_with(frames, || MaybeUninit::uninit());
            let bookkeeping = NonNull::from(bookkeeping.as_mut_slice());

            let mut area: Area<DEFAULT_MAX_ORDER> = Area::from_parts(
                PhysicalAddress::new(0x0)..PhysicalAddress::new(frames * PAGE_SIZE),
                bookkeeping,
                PAGE_SIZE,
            );

            prop_assert_eq!(
                area.area,
                PhysicalAddress::new(0x0)..PhysicalAddress::new(frames * PAGE_SIZE)
            );

            let mut frames_remaining = frames;
            while frames_remaining > 0 {
                // clamp the order we calculate at the max possible order
                let chunk_order = cmp::min(ilog2(frames_remaining) as usize, DEFAULT_MAX_ORDER - 1);

                let chunk = area.free_lists[chunk_order].pop_back();
                prop_assert!(chunk.is_some(), "expected chunk of order {chunk_order}");

                frames_remaining -= 1 << chunk_order;
            }
            prop_assert!(area.free_lists.iter().all(|list| list.is_empty()));
        }

        #[test]
        fn alloc_dealloc(frames in 1usize..50_000usize, alloc_frames in 1usize..50_000usize) {
            const PAGE_SIZE: usize = 4096;

            let mut bookkeeping1: Vec<MaybeUninit<Frame>> = Vec::with_capacity(frames);
            bookkeeping1.resize_with(frames, || MaybeUninit::uninit());
            let bookkeeping1 = NonNull::from(bookkeeping1.as_mut_slice());

            let mut area1: Area<DEFAULT_MAX_ORDER> = Area::from_parts(
                PhysicalAddress::new(0x0)..PhysicalAddress::new(frames * PAGE_SIZE),
                bookkeeping1,
                PAGE_SIZE,
            );

            let mut bookkeeping2: Vec<MaybeUninit<Frame>> = Vec::with_capacity(frames);
            bookkeeping2.resize_with(frames, || MaybeUninit::uninit());
            let bookkeeping2 = NonNull::from(bookkeeping2.as_mut_slice());

            let mut area2: Area<DEFAULT_MAX_ORDER> = Area::from_parts(
                PhysicalAddress::new(0x0)..PhysicalAddress::new(frames * PAGE_SIZE),
                bookkeeping2,
                PAGE_SIZE,
            );

            let alloc_frames = prev_power_of_two(cmp::min(cmp::min(frames, alloc_frames), 1 << (DEFAULT_MAX_ORDER - 1)));
            let layout = Layout::from_size_align(PAGE_SIZE * alloc_frames, PAGE_SIZE).unwrap();

            let mut out = List::new();
            area2.allocate(layout, &mut out).unwrap();

            prop_assert_eq!(out.len(), alloc_frames);

            out.iter_mut().for_each(|frame| frame.refcount.store(0, Ordering::Release));
            unsafe {area2.deallocate(out.pop_back().unwrap(), layout);}

            for order in 0..DEFAULT_MAX_ORDER {
                prop_assert!(area1.free_lists[order].iter().eq(area2.free_lists[order].iter()), "free lists of order {order} differ");
            }
        }
    }

    #[test]
    fn tt() {
        let frames = 14336;
        let alloc_frames = 4;

        const PAGE_SIZE: usize = 4096;

        let mut bookkeeping1: Vec<MaybeUninit<Frame>> = Vec::with_capacity(frames);
        bookkeeping1.resize_with(frames, || MaybeUninit::uninit());
        let bookkeeping1 = NonNull::from(bookkeeping1.as_mut_slice());

        let mut area1: Area<DEFAULT_MAX_ORDER> = Area::from_parts(
            PhysicalAddress::new(0x0)..PhysicalAddress::new(frames * PAGE_SIZE),
            bookkeeping1,
            PAGE_SIZE,
        );

        let mut bookkeeping2: Vec<MaybeUninit<Frame>> = Vec::with_capacity(frames);
        bookkeeping2.resize_with(frames, || MaybeUninit::uninit());
        let bookkeeping2 = NonNull::from(bookkeeping2.as_mut_slice());

        let mut area2: Area<DEFAULT_MAX_ORDER> = Area::from_parts(
            PhysicalAddress::new(0x0)..PhysicalAddress::new(frames * PAGE_SIZE),
            bookkeeping2,
            PAGE_SIZE,
        );

        println!("{area2:?}");

        let alloc_frames = prev_power_of_two(cmp::min(
            cmp::min(frames, alloc_frames),
            1 << (DEFAULT_MAX_ORDER - 1),
        ));
        let layout = Layout::from_size_align(PAGE_SIZE * alloc_frames, PAGE_SIZE).unwrap();

        println!("attempting to alloc {layout:?}");
        let mut out = List::new();
        area2.allocate(layout, &mut out).unwrap();

        // println!("{area2:?}");

        assert_eq!(out.len(), alloc_frames);

        out.iter_mut()
            .for_each(|frame| frame.refcount.store(0, Ordering::Release));

        let out_: Vec<_> = out.iter().collect();

        println!("{out_:?}");

        unsafe {
            area2.deallocate(out.pop_back().unwrap(), layout);
        }

        println!("{area1:?}");
        println!("{area2:?}");

        for order in 0..DEFAULT_MAX_ORDER {
            assert!(
                area1.free_lists[order]
                    .iter()
                    .eq(area2.free_lists[order].iter()),
                "free lists of order {order} differ"
            );
        }
    }
}
