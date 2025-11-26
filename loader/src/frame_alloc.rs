// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::{iter, slice};

use kmem::arch::Arch;
use kmem::{AddressRangeExt, AllocError, PhysicalAddress};
use spin::Mutex;

pub struct FrameAllocator<'a>(Mutex<FrameAllocatorInner<'a>>);

struct FrameAllocatorInner<'a> {
    regions: &'a [Range<PhysicalAddress>],
    // offset from the top of memory regions
    offset: usize,
}

impl<'a> FrameAllocator<'a> {
    /// Create a new frame allocator over a given set of physical memory regions.
    #[must_use]
    pub fn new(regions: &'a [Range<PhysicalAddress>]) -> Self {
        Self(Mutex::new(FrameAllocatorInner { regions, offset: 0 }))
    }

    #[must_use]
    pub fn free_regions(&self) -> FreeRegions<'_> {
        let inner = self.0.lock();

        FreeRegions {
            offset: inner.offset,
            inner: inner.regions.iter().rev().cloned(),
        }
    }

    #[must_use]
    pub fn used_regions(&self) -> UsedRegions<'_> {
        let inner = self.0.lock();

        UsedRegions {
            offset: inner.offset,
            inner: inner.regions.iter().rev().cloned(),
        }
    }

    pub fn usage(&self) -> usize {
        let inner = self.0.lock();
        inner.offset
    }
}

impl<'a> FrameAllocatorInner<'a> {
    fn allocate_contiguous(&mut self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let requested_size = layout.pad_to_align().size();
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.len();

            // only consider regions that we haven't already exhausted
            if offset < region_size {
                // Allocating a contiguous range has different requirements than "regular" allocation
                // contiguous are rare and often happen in very critical paths where e.g. virtual
                // memory is not available yet. So we rather waste some memory than outright crash.
                if region_size - offset < requested_size {
                    log::warn!(
                        "Skipped memory region {region:?} since it was too small to fulfill request for {requested_size} bytes. Wasted {} bytes in the process...",
                        region_size - offset
                    );

                    self.offset += region_size - offset;
                    offset = 0;
                    continue;
                }

                let frame = region.end.sub(offset + requested_size);
                self.offset += requested_size;
                return Ok(frame);
            }

            offset -= region_size;
        }

        Err(AllocError)
    }
}

unsafe impl<A: Arch> kmem::FrameAllocator<A> for FrameAllocator<'_> {
    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>) {
        let frame_size = unsafe { NonZeroUsize::new_unchecked(A::PAGE_SIZE) };
        (frame_size, Some(frame_size))
    }

    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        debug_assert_eq!(
            layout.align(),
            A::PAGE_SIZE,
            "FrameAllocator only supports page-aligned allocations"
        );

        self.0.lock().allocate_contiguous(layout)
    }

    unsafe fn deallocate(&self, _block: PhysicalAddress, _layout: Layout) {
        unreachable!("FrameAllocator does not support deallocation");
    }
}

pub struct FreeRegions<'a> {
    offset: usize,
    inner: iter::Cloned<iter::Rev<slice::Iter<'a, Range<PhysicalAddress>>>>,
}

impl Iterator for FreeRegions<'_> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut region = self.inner.next()?;
            // keep advancing past already fully used memory regions
            let region_size = region.len();

            if self.offset >= region_size {
                self.offset -= region_size;
                continue;
            } else if self.offset > 0 {
                region.end = region.end.sub(self.offset);
                self.offset = 0;
            }

            return Some(region);
        }
    }
}

pub struct UsedRegions<'a> {
    offset: usize,
    inner: iter::Cloned<iter::Rev<slice::Iter<'a, Range<PhysicalAddress>>>>,
}

impl Iterator for UsedRegions<'_> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut region = self.inner.next()?;

        if self.offset >= region.len() {
            Some(region)
        } else if self.offset > 0 {
            region.start = region.end.sub(self.offset);
            self.offset = 0;

            Some(region)
        } else {
            None
        }
    }
}
