// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod area;
mod area_selection;

use core::alloc::Layout;
use core::cell::RefCell;
use core::cmp;
use core::ops::Range;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

use cordyceps::List;
use cpu_local::collection::CpuLocal;
use fallible_iterator::FallibleIterator;
use lock_api::Mutex;
use smallvec::SmallVec;

use crate::address_space::RawAddressSpace;
use crate::frame_alloc::area::Area;
use crate::frame_alloc::area_selection::select_areas;
use crate::{Frame, PhysicalAddress};

#[derive(Debug)]
pub struct AllocError;

pub unsafe trait FrameAllocator: Send + Sync + 'static {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[Frame]>, AllocError>;
    unsafe fn deallocate(&self, block: NonNull<Frame>, layout: Layout);
    fn page_size(&self) -> usize;
}

const MAX_FRAMES_IN_CACHE: usize = 256;

pub struct FrameAlloc<L: lock_api::RawMutex, A: RawAddressSpace> {
    areas: Mutex<L, SmallVec<[Area<A>; 4]>>,
    cpu_local_cache: CpuLocal<RefCell<List<Frame>>>,
    max_alignment_hint: AtomicUsize,
}

impl<L: lock_api::RawMutex, A: RawAddressSpace> FrameAlloc<L, A> {
    pub fn new(allocatable_regions: SmallVec<[Range<PhysicalAddress>; 4]>) -> crate::Result<Self> {
        let mut max_alignment_hint = 0;
        let mut areas = SmallVec::new();

        let mut selections = select_areas::<A>(allocatable_regions);
        while let Some(selection) = selections.next()? {
            let area = Area::new(selection.area, selection.bookkeeping);
            max_alignment_hint = cmp::max(max_alignment_hint, area.max_alignment_hint());
            areas.push(area);
        }

        Ok(Self {
            areas: Mutex::new(areas),
            cpu_local_cache: CpuLocal::new(),
            max_alignment_hint: AtomicUsize::new(max_alignment_hint),
        })
    }

    pub fn max_alignment_hint(&self) -> usize {
        self.max_alignment_hint.load(Ordering::Relaxed)
    }

    fn allocate_local(&self, layout: Layout) -> Option<NonNull<Frame>> {
        if layout.size() == A::PAGE_SIZE && layout.align() == A::PAGE_SIZE {
            let mut cache = self.cpu_local_cache.get_or_default().borrow_mut();
            cache.pop_back()
        } else {
            None
        }
    }

    fn deallocate_local(&self, block: NonNull<Frame>, layout: Layout) -> bool {
        if layout.size() == A::PAGE_SIZE && layout.align() == A::PAGE_SIZE {
            let mut cache = self.cpu_local_cache.get_or_default().borrow_mut();

            if cache.len() < MAX_FRAMES_IN_CACHE {
                cache.push_back(block);
                return true;
            }
        }

        false
    }
}

unsafe impl<L: lock_api::RawMutex + Send + Sync, A: RawAddressSpace + Send + Sync> FrameAllocator
    for &'static FrameAlloc<L, A>
{
    fn allocate(&self, layout: Layout) -> Result<NonNull<[Frame]>, AllocError> {
        // attempt to allocate from the CPU-local cache first
        if let Some(frame) = self.allocate_local(layout) {
            return Ok(NonNull::slice_from_raw_parts(frame.cast(), 1));
        }

        let mut areas = self.areas.lock();
        for area in areas.iter_mut() {
            if let Ok(frames) = area.allocate(layout) {
                return Ok(frames);
            }
        }

        Err(AllocError)
    }

    unsafe fn deallocate(&self, block: NonNull<Frame>, layout: Layout) {
        // attempt to place the frame into the CPU-local cache first
        if self.deallocate_local(block, layout) {
            return;
        }

        let mut areas = self.areas.lock();
        for area in areas.iter_mut() {
            let block_ = unsafe { block.as_ref() };

            if area.contains_frame(block_.addr()) {
                unsafe { area.deallocate(block, layout) };

                self.max_alignment_hint
                    .fetch_max(area.max_alignment_hint(), Ordering::Relaxed);

                return;
            }
        }

        unreachable!();
    }

    fn page_size(&self) -> usize {
        A::PAGE_SIZE
    }
}
