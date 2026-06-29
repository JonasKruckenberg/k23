// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::range::Range;
use core::{cmp, fmt, mem, slice};

use cordyceps::List;
use fallible_iterator::FallibleIterator;
use mem_core::{AddressRangeExt, PhysMap, PhysicalAddress};
use smallvec::SmallVec;

use super::frame::FrameInfo;
use crate::arch;

const ARENA_PAGE_BOOKKEEPING_SIZE: usize = size_of::<FrameInfo>();
const MAX_WASTED_ARENA_BYTES: usize = 0x8_4000; // 528 KiB
const MAX_ORDER: usize = 11;

pub struct Arena {
    free_lists: [List<FrameInfo>; MAX_ORDER],
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
    pub fn from_selection(selection: ArenaSelection, physmap: &PhysMap) -> Self {
        debug_assert!(selection.bookkeeping.len() >= bookkeeping_size(selection.arena.len()));

        // Safety: arena selection has ensured the region is valid
        let slots: &mut [MaybeUninit<FrameInfo>] = unsafe {
            let ptr = physmap
                .phys_to_virt(selection.bookkeeping.start)
                .as_mut_ptr()
                .cast();

            slice::from_raw_parts_mut(
                ptr,
                selection.bookkeeping.len() / ARENA_PAGE_BOOKKEEPING_SIZE,
            )
        };

        let total_frames = selection.arena.len() / arch::PAGE_SIZE;

        // Default-deny: every slot in the hull starts wired (not on any
        // free list). Hole and bookkeeping pages stay this way for life;
        // the loop below demotes pages inside `allocatable_regions` to
        // free.
        for (idx, slot) in slots[..total_frames].iter_mut().enumerate() {
            slot.write(FrameInfo::new_wired(
                selection.arena.start.add(idx * arch::PAGE_SIZE),
            ));
        }

        let mut free_lists = [const { List::new() }; MAX_ORDER];
        let mut max_order = 0;
        let mut free_frames = 0;

        // Hand free pages to the buddy in chunks bounded by their sub-range.
        // No block straddles a hole, so splits stay correct without new checks.
        for region in &selection.allocatable_regions {
            let mut addr = region.start;
            let mut remaining = region.len();
            while remaining > 0 {
                let size = cmp::min(
                    cmp::min(
                        addr.get() & addr.get().wrapping_neg(),
                        prev_power_of_two(remaining),
                    ),
                    arch::PAGE_SIZE << (MAX_ORDER - 1),
                );
                let order = (size / arch::PAGE_SIZE).trailing_zeros() as usize;
                free_frames += size / arch::PAGE_SIZE;
                max_order = cmp::max(max_order, order);

                let idx = addr.offset_from_unsigned(selection.arena.start) / arch::PAGE_SIZE;
                // SAFETY: pass 1 initialized this slot.
                let frame = unsafe { slots[idx].assume_init_mut() };
                frame.mark_as_free_for_freelist();
                free_lists[order].push_back(NonNull::from(frame));

                addr = addr.add(size);
                remaining -= size;
            }
        }

        Self {
            range: selection.arena,
            slots,
            free_lists,
            max_order,
            used_frames: total_frames - free_frames,
            total_frames,
        }
    }

    pub fn max_alignment(&self) -> usize {
        arch::PAGE_SIZE << self.max_order
    }

    pub fn allocate_one(&mut self) -> Option<NonNull<FrameInfo>> {
        let (frame_order, mut frame) = self.free_lists[..=self.max_order]
            .iter_mut()
            .enumerate()
            .find_map(|(i, list)| list.pop_back().map(|area| (i, area)))?;

        for order in (1..frame_order + 1).rev() {
            // Safety: we just allocated the frame
            let frame = unsafe { frame.as_mut() };

            let buddy_addr = frame.addr().add(arch::PAGE_SIZE << (order - 1));

            let buddy = self
                .find_specific(buddy_addr)
                .unwrap()
                .write(FrameInfo::new_free(buddy_addr))
                .into();

            self.free_lists[order - 1].push_back(buddy);
        }

        Some(frame)
    }

    pub fn allocate_contiguous(&mut self, layout: Layout) -> Option<List<FrameInfo>> {
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
            // Safety: we just allocated the frame
            let frame = unsafe { frame.as_mut() };

            let buddy_addr = frame.addr().add(arch::PAGE_SIZE << (order - 1));

            let buddy = self
                .find_specific(buddy_addr)
                .unwrap()
                .write(FrameInfo::new_free(buddy_addr))
                .into();

            self.free_lists[order - 1].push_back(buddy);
        }

        // Initialize all frame structs
        // The base frame we pulled from the freelist is already correctly initialized, but all following
        // frames of its buddy "block" are left uninitialized, so we need to do that now.
        let frames = {
            let uninit: &mut [MaybeUninit<FrameInfo>] =
                // Safety: we just allocate the frames
                unsafe { slice::from_raw_parts_mut(frame.cast().as_ptr(), size_frames) };

            // Safety: we just allocate `frame`
            let base = unsafe { frame.as_ref().addr() };

            uninit.iter_mut().enumerate().map(move |(idx, slot)| {
                NonNull::from(slot.write(FrameInfo::new_free(base.add(idx * arch::PAGE_SIZE))))
            })
        };

        self.used_frames += size_frames;
        Some(List::from_iter(frames))
    }

    #[inline]
    fn find_specific(&mut self, addr: PhysicalAddress) -> Option<&mut MaybeUninit<FrameInfo>> {
        let index = addr.offset_from_unsigned(self.range.start) / arch::PAGE_SIZE;
        self.slots.get_mut(index)
    }
}

// === Arena selection ===

pub fn select_arenas(free_regions: loader_api::MemoryRegions) -> ArenaSelections {
    ArenaSelections {
        free_regions,
        wasted_bytes: 0,
    }
}

#[derive(Debug)]
pub struct ArenaSelection {
    pub arena: Range<PhysicalAddress>,
    pub allocatable_regions: SmallVec<[Range<PhysicalAddress>; 4]>,
    pub bookkeeping: Range<PhysicalAddress>,
    pub wasted_bytes: usize,
}

#[derive(Debug)]
pub struct SelectionError {
    pub range: Range<PhysicalAddress>,
}

pub struct ArenaSelections {
    free_regions: loader_api::MemoryRegions,
    wasted_bytes: usize,
}

impl FallibleIterator for ArenaSelections {
    type Item = ArenaSelection;
    type Error = SelectionError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let Some(seed) = self.free_regions.pop() else {
            return Ok(None);
        };

        let mut hull = seed.range;
        let mut regions: SmallVec<[Range<PhysicalAddress>; 4]> = SmallVec::new();
        regions.push(seed.range);

        while let Some(region) = self.free_regions.pop() {
            debug_assert!(!hull.overlaps(&region.range));

            let hole_pages = if hull.end <= region.range.start {
                region.range.start.offset_from_unsigned(hull.end)
            } else {
                debug_assert!(region.range.end <= hull.start);
                hull.start.offset_from_unsigned(region.range.end)
            } / arch::PAGE_SIZE;

            let waste = ARENA_PAGE_BOOKKEEPING_SIZE * hole_pages;
            if self.wasted_bytes + waste > MAX_WASTED_ARENA_BYTES {
                self.free_regions.push(region);
                break;
            }
            self.wasted_bytes += waste;
            hull.start = cmp::min(hull.start, region.range.start);
            hull.end = cmp::max(hull.end, region.range.end);
            regions.push(region.range);
        }

        let arena = hull.align_in(arch::PAGE_SIZE);
        let mut regions: SmallVec<[Range<PhysicalAddress>; 4]> = regions
            .into_iter()
            .map(|r| r.align_in(arch::PAGE_SIZE))
            .filter(|r| !r.is_empty())
            .collect();
        regions.sort_unstable_by_key(|r| r.start);

        // Anchor bookkeeping to the largest sub-range. `arena.end` may sit
        // inside a merged-over hole (kernel image, debuginfo blob); writing
        // metadata there would clobber mapped kernel memory.
        let need = bookkeeping_size(arena.len());
        let host = regions.iter_mut().max_by_key(|r| r.len());
        let Some(host) = host.filter(|r| r.len() >= need) else {
            return Err(SelectionError { range: arena });
        };
        let bookkeeping_start = host.end.sub(need).align_down(arch::PAGE_SIZE);
        let bookkeeping = Range::from(bookkeeping_start..host.end);
        host.end = bookkeeping_start;
        regions.retain(|r| !r.is_empty());

        Ok(Some(ArenaSelection {
            arena,
            allocatable_regions: regions,
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
