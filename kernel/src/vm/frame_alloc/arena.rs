// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::frame::FrameInfo;
use crate::arch;
use crate::vm::address::{AddressRangeExt, PhysicalAddress, VirtualAddress};
use crate::vm::bootstrap_alloc::FreeRegions;
use core::alloc::Layout;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::range::Range;
use core::{cmp, fmt, mem, slice};
use fallible_iterator::FallibleIterator;

const ARENA_PAGE_BOOKKEEPING_SIZE: usize = size_of::<FrameInfo>();
const MAX_WASTED_ARENA_BYTES: usize = 0x8_4000; // 528 KiB
const MAX_ORDER: usize = 11;

pub struct Arena {
    free_lists: [linked_list::List<FrameInfo>; MAX_ORDER],
    range: Range<PhysicalAddress>,
    slots: &'static mut [MaybeUninit<FrameInfo>],
    max_order: usize,
    used_frames: usize,
    total_frames: usize,
}

impl fmt::Debug for Arena {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arena")
            .field("range", &self.range)
            .field(
                "slots",
                &format_args!("&[MaybeUninit<FrameInner>; {}]", self.slots.len()),
            )
            .field_with("free_lists", |f| {
                let mut f = f.debug_map();
                for (order, l) in self.free_lists.iter().enumerate() {
                    f.key(&order);
                    f.value_with(|f| f.debug_list().entries(l.iter()).finish());
                }
                f.finish()
            })
            .field("max_order", &self.max_order)
            .field("used_frames", &self.used_frames)
            .field("total_frames", &self.total_frames)
            .finish()
    }
}

impl Arena {
    pub fn from_selection(selection: ArenaSelection) -> Self {
        debug_assert!(selection.bookkeeping.size() >= bookkeeping_size(selection.arena.size()));

        let slots: &mut [MaybeUninit<FrameInfo>] = unsafe {
            let ptr = VirtualAddress::from_phys(selection.bookkeeping.start)
                .unwrap()
                .as_mut_ptr()
                .cast();

            slice::from_raw_parts_mut(
                ptr,
                selection.bookkeeping.size() / ARENA_PAGE_BOOKKEEPING_SIZE,
            )
        };

        let mut remaining_bytes = selection.arena.size();
        let mut addr = selection.arena.start;
        let mut total_frames = 0;
        let mut max_order = 0;
        let mut free_lists = [const { linked_list::List::new() }; MAX_ORDER];

        while remaining_bytes > 0 {
            let lowbit = addr.get() & (!addr.get() + 1);

            let size = cmp::min(
                cmp::min(lowbit, prev_power_of_two(remaining_bytes)),
                arch::PAGE_SIZE << (MAX_ORDER - 1),
            );

            let size_pages = size / arch::PAGE_SIZE;
            let order = size_pages.trailing_zeros() as usize;
            total_frames += size_pages;
            max_order = cmp::max(max_order, order);

            {
                debug_assert!(addr.is_aligned_to(arch::PAGE_SIZE));
                let offset = addr.checked_sub_addr(selection.arena.start).unwrap();
                let idx = offset / arch::PAGE_SIZE;

                let frame = slots[idx].write(FrameInfo::new(addr)).into();
                free_lists[order].push_back(frame);
            }

            addr = addr.checked_add(size).unwrap();
            remaining_bytes -= size;
        }

        // Make sure we've accounted for all frames
        debug_assert_eq!(total_frames, selection.arena.size() / arch::PAGE_SIZE);

        Self {
            range: selection.arena,
            slots,
            free_lists,
            max_order,
            used_frames: 0,
            total_frames,
        }
    }
    
    pub fn max_alignment(&self) -> usize {
        PAGE_SIZE << self.max_order
    }

    pub fn allocate_one(&mut self) -> Option<NonNull<FrameInfo>> {
        let (frame_order, mut frame) = self.free_lists[..=self.max_order]
            .iter_mut()
            .enumerate()
            .find_map(|(i, list)| list.pop_back().map(|area| (i, area)))?;

        for order in (1..frame_order + 1).rev() {
            let frame = unsafe { frame.as_mut() };

            let buddy_addr = frame
                .addr()
                .checked_add(arch::PAGE_SIZE << (order - 1))
                .unwrap();

            let buddy = self
                .find_specific(buddy_addr)
                .unwrap()
                .write(FrameInfo::new(buddy_addr))
                .into();

            self.free_lists[order - 1].push_back(buddy);
        }

        Some(frame)
    }

    pub fn allocate_contiguous(&mut self, layout: Layout) -> Option<linked_list::List<FrameInfo>> {
        assert!(layout.align() >= arch::PAGE_SIZE);
        assert!(layout.size() >= arch::PAGE_SIZE);

        let size = cmp::max(layout.size().next_power_of_two(), layout.align());
        let size_frames = size / arch::PAGE_SIZE;
        let min_order = size_frames.trailing_zeros() as usize;

        // locate a free area of the requested alignment from the freelists
        let (frame_order, mut frame) = self.free_lists[..=self.max_order]
            .iter_mut()
            .enumerate()
            .skip(min_order)
            .find_map(|(i, list)| list.pop_back().map(|area| (i, area)))?;

        // if the free area we found was of higher order (ie larger) that we requested
        // we need to split it up
        for order in (min_order + 1..frame_order + 1).rev() {
            let frame = unsafe { frame.as_mut() };

            let buddy_addr = frame
                .addr()
                .checked_add(arch::PAGE_SIZE << (order - 1))
                .unwrap();

            let buddy = self
                .find_specific(buddy_addr)
                .unwrap()
                .write(FrameInfo::new(buddy_addr))
                .into();

            self.free_lists[order - 1].push_back(buddy);
        }

        // Initialize all frame structs
        // The base frame we pulled from the freelist is already correctly initialized, but all following
        // frames of its buddy "block" are left uninitialized, so we need to do that now.
        let frames = {
            let uninit: &mut [MaybeUninit<FrameInfo>] =
                unsafe { slice::from_raw_parts_mut(frame.cast().as_ptr(), size_frames) };

            let base = unsafe { frame.as_ref().addr() };

            uninit.iter_mut().enumerate().map(move |(idx, slot)| {
                NonNull::from(slot.write(FrameInfo::new(
                    base.checked_add(idx * arch::PAGE_SIZE).unwrap(),
                )))
            })
        };

        self.used_frames += size_frames;
        Some(linked_list::List::from_iter(frames))
    }

    #[inline]
    fn find_specific(&mut self, addr: PhysicalAddress) -> Option<&mut MaybeUninit<FrameInfo>> {
        let index = addr.checked_sub_addr(self.range.start).unwrap() / arch::PAGE_SIZE;
        self.slots.get_mut(index)
    }
}

// =============================================================================
// Arena selection
// =============================================================================

pub fn select_arenas(free_regions: FreeRegions) -> ArenaSelections {
    ArenaSelections {
        inner: free_regions,
        wasted_bytes: 0,
    }
}

#[derive(Debug)]
pub struct ArenaSelection {
    pub arena: Range<PhysicalAddress>,
    pub bookkeeping: Range<PhysicalAddress>,
    pub wasted_bytes: usize,
}

#[derive(Debug)]
pub struct SelectionError {
    pub range: Range<PhysicalAddress>,
}

pub struct ArenaSelections<'a> {
    inner: FreeRegions<'a>,
    wasted_bytes: usize,
}

impl FallibleIterator for ArenaSelections<'_> {
    type Item = ArenaSelection;
    type Error = SelectionError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let Some(mut arena) = self.inner.next() else {
            return Ok(None);
        };

        for region in self.inner.by_ref() {
            let pages_in_hole = region.start.checked_sub_addr(arena.end).unwrap() / arch::PAGE_SIZE;
            let waste_from_hole = ARENA_PAGE_BOOKKEEPING_SIZE * pages_in_hole;

            if self.wasted_bytes + waste_from_hole > MAX_WASTED_ARENA_BYTES {
                break;
            } else {
                self.wasted_bytes += waste_from_hole;
                arena.end = region.end;
            }
        }

        let mut aligned = arena.checked_align_in(arch::PAGE_SIZE).unwrap();
        let bookkeeping_size = bookkeeping_size(aligned.size());

        // We can't use empty arenas anyway
        if aligned.is_empty() {
            log::error!("arena is too small");
            return Err(SelectionError { range: aligned });
        }

        let bookkeeping_start = aligned
            .end
            .checked_sub(bookkeeping_size)
            .unwrap()
            .align_down(arch::PAGE_SIZE);

        // The arena has no space to hold its own bookkeeping
        if bookkeeping_start < aligned.start {
            log::error!("arena is too small");
            return Err(SelectionError { range: aligned });
        }

        let bookkeeping = Range::from(bookkeeping_start..aligned.end);
        aligned.end = bookkeeping.start;

        Ok(Some(ArenaSelection {
            arena: aligned,
            bookkeeping,
            wasted_bytes: mem::take(&mut self.wasted_bytes),
        }))
    }
}

fn prev_power_of_two(num: usize) -> usize {
    1 << (usize::BITS as usize - num.leading_zeros() as usize - 1)
}

fn bookkeeping_size(region_size: usize) -> usize {
    (region_size / arch::PAGE_SIZE) * ARENA_PAGE_BOOKKEEPING_SIZE
}
