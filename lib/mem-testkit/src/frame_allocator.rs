// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::alloc::Layout;
use std::collections::BTreeSet;
use std::range::Range;
use std::sync::Mutex;

use mem_core::arch::Arch;
use mem_core::{AddressRangeExt, AllocError, FrameAllocator, PhysicalAddress};

/// A deliberately simple frame allocator for exercising `mem-core` in tests.
///
/// Unlike the bump allocators used during real boot, this one tracks every free frame
/// individually and *supports deallocation*, so tests can drive map/unmap cycles and assert
/// that frames are actually reclaimed (and that `mem-core` never double-frees). It is not
/// optimized for anything — it linearly scans a [`BTreeSet`] of free frame bases — which is
/// fine for the small machines tests construct.
#[derive(Debug)]
pub struct TestFrameAllocator {
    granule: usize,
    /// Base addresses of all currently-free frames, kept sorted so allocation is deterministic
    /// (lowest free frame / run first).
    free: Mutex<BTreeSet<PhysicalAddress>>,
}

impl TestFrameAllocator {
    /// Carves the given physical memory `regions` into frames of `A::GRANULE_SIZE` and marks
    /// them all free. Regions are rounded inward to whole frames; partial frames are discarded.
    pub fn new<A: Arch>(regions: impl IntoIterator<Item = Range<PhysicalAddress>>) -> Self {
        let granule = A::GRANULE_SIZE;

        let mut free = BTreeSet::new();
        for region in regions {
            let region = region.align_in(granule);

            let mut frame = region.start;
            while frame < region.end {
                free.insert(frame);
                frame = frame.add(granule);
            }
        }

        Self {
            granule,
            free: Mutex::new(free),
        }
    }

    /// Returns the number of frames that are currently free.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock has been poisoned by a panic in another allocator call.
    pub fn free_frames(&self) -> usize {
        self.free.lock().unwrap().len()
    }
}

// Safety: every handed-out frame comes from the machine's backing memory and is removed from the
// free set, so distinct allocations never overlap and stay valid until deallocated.
unsafe impl FrameAllocator for TestFrameAllocator {
    fn allocate(
        &self,
        layout: Layout,
    ) -> Result<impl ExactSizeIterator<Item = Range<PhysicalAddress>>, AllocError> {
        if layout.size() == 0 {
            return Err(AllocError);
        }
        // Each block we hand back is a single frame, so it can only honor alignments up to one
        // frame. Larger alignments must go through `allocate_contiguous`.
        assert!(
            layout.align() <= self.granule,
            "discontiguous allocation supports at most frame alignment ({:#x}), got {:#x}",
            self.granule,
            layout.align()
        );

        let nframes = layout.size().div_ceil(self.granule);
        let mut free = self.free.lock().unwrap();

        if free.len() < nframes {
            return Err(AllocError);
        }

        // Take the `nframes` lowest free frames, one block each.
        let mut blocks = Vec::with_capacity(nframes);
        for _ in 0..nframes {
            let frame = *free.iter().next().expect("checked free.len() above");
            free.remove(&frame);
            blocks.push(Range::from(frame..frame.add(self.granule)));
        }

        Ok(blocks.into_iter())
    }

    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        if layout.size() == 0 {
            return Err(AllocError);
        }

        let align = layout.align().max(self.granule);
        let nframes = layout.size().div_ceil(self.granule);
        let mut free = self.free.lock().unwrap();

        // Find the lowest `align`-aligned frame that begins a run of `nframes` free frames.
        let base = free
            .iter()
            .copied()
            .find(|&start| {
                start.is_aligned_to(align)
                    && (0..nframes).all(|i| free.contains(&start.add(i * self.granule)))
            })
            .ok_or(AllocError)?;

        for i in 0..nframes {
            free.remove(&base.add(i * self.granule));
        }

        Ok(base)
    }

    unsafe fn deallocate(&self, block: PhysicalAddress, layout: Layout) {
        let nframes = layout.size().div_ceil(self.granule);
        let mut free = self.free.lock().unwrap();

        for i in 0..nframes {
            let frame = block.add(i * self.granule);
            debug_assert!(
                frame.is_aligned_to(self.granule),
                "deallocated block {block:?} is not frame-aligned"
            );
            // Catches `mem-core` double-frees / frees of never-allocated frames.
            let newly_freed = free.insert(frame);
            debug_assert!(newly_freed, "double free of frame {frame:?}");
        }
    }
}
