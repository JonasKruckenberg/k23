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
use core::ptr::NonNull;

pub use area_selection::{AreaSelection, SelectionError, select_areas};
use cordyceps::List;
use k23_cpu_local::collection::CpuLocal;
use lock_api::Mutex;
use smallvec::SmallVec;

use crate::RawAddressSpace;
use crate::frame::Frame;
use crate::frame_allocator::area::Area;

/// The `AllocError` error indicates an allocation failure that may be due to resource exhaustion or
/// to something wrong when combining the given input arguments with this frame allocator.
#[derive(Debug)]
pub struct AllocError;

const MAX_FRAMES_IN_CACHE: usize = 256;

/// A frame allocator that manages physical memory frames across multiple areas with CPU-local caching.
///
/// The frame size and other hardware specific details are configured through the first generic
/// parameter.
///
/// The [`lock_api::RawMutex`] implementation used to protect the area set can be chosen through
/// the second generic parameter.
pub struct FrameAllocator<A: RawAddressSpace, L: lock_api::RawMutex> {
    /// A CPU-local cache of up-to [`MAX_FRAMES_IN_CACHE`] frames.
    /// Will be checked first before falling back to the global areas to reduce
    /// lock contention.
    cpu_local_cache: CpuLocal<RefCell<List<Frame>>>,

    /// The global set of areas.
    areas: Mutex<L, SmallVec<[Area<A>; 4]>>,

    /// The absolute upper bound on the alignment supported by this allocator. The actual
    /// maximal alignment might be lower after allocations.
    max_alignment_hint: usize,
}

impl<A: RawAddressSpace, L: lock_api::RawMutex> FromIterator<Area<A>> for FrameAllocator<A, L> {
    /// Construct a new `FrameAllocator` from the provided areas.
    fn from_iter<T: IntoIterator<Item = Area<A>>>(iter: T) -> Self {
        let mut max_alignment_hint = 0;

        let areas = iter
            .into_iter()
            .inspect(|area| {
                max_alignment_hint = cmp::max(area.max_alignment_hint(), max_alignment_hint);
            })
            .collect();

        Self {
            areas: Mutex::new(areas),
            cpu_local_cache: CpuLocal::new(),
            max_alignment_hint,
        }
    }
}

impl<A: RawAddressSpace, L: lock_api::RawMutex> FrameAllocator<A, L> {
    /// Construct a new `FrameAllocator` from the provided areas.
    pub fn new(areas: SmallVec<[Area<A>; 4]>) -> Self {
        let mut max_alignment_hint = 0;

        for area in &areas {
            max_alignment_hint = cmp::max(area.max_alignment_hint(), max_alignment_hint);
        }

        Self {
            areas: Mutex::new(areas),
            cpu_local_cache: CpuLocal::new(),
            max_alignment_hint,
        }
    }

    /// The absolute upper bound on the alignment supported by this allocator. The actual
    /// maximal alignment might be lower.
    ///
    /// Note that allocations might still fail using the alignment returned by this method, it serves
    /// merely as a performance optimization hint.
    pub fn max_alignment_hint(&self) -> usize {
        self.max_alignment_hint
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
    pub fn allocate(&self, layout: Layout) -> Result<NonNull<[Frame]>, AllocError> {
        // attempt to allocate from the CPU-local cache first
        if let Some(frame) = self.allocate_local(layout) {
            return Ok(NonNull::slice_from_raw_parts(frame.cast(), 1));
        }

        let mut areas = self.areas.lock();
        for area in areas.iter_mut() {
            let res = area.allocate(layout);

            if let Ok(frames) = res {
                return Ok(frames);
            }
        }

        Err(AllocError)
    }

    /// Deallocates a block of frames referenced by `block`.
    ///
    /// # Safety
    ///
    /// * `block` must denote a block of frames *currently allocated* via this allocator, and
    /// * `layout` must *fit* that block of frames.
    pub unsafe fn deallocate(&self, block: NonNull<Frame>, layout: Layout) {
        // attempt to place the frame into the CPU-local cache first
        if self.deallocate_local(block, layout) {
            return;
        }

        let mut areas = self.areas.lock();
        for area in areas.iter_mut() {
            // Safety: the caller must ensure the block is currently allocated (and therefore initialized)
            let block_ = unsafe { block.as_ref() };

            if area.contains_frame(block_.addr()) {
                // Safety: we have determined above the block "belongs" to the area
                // but the caller must ensure the block is allocated and fitting the layout
                unsafe { area.deallocate(block, layout) };
                return;
            }
        }

        unreachable!();
    }

    pub fn assert_valid(&mut self, ctx: &str) {
        if let Some(areas) = self.areas.try_lock() {
            for area in areas.iter() {
                area.assert_valid(ctx);
            }
        } else {
            // TODO warn about locked areas
        }

        // NB: mutable iterator here, not because we need to mutate the lists
        // but because must guarantee no other CPU is currently looking at (and potentially mutating)
        // these lists.
        for cache in &mut self.cpu_local_cache {
            cache.borrow().assert_valid();
        }
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
