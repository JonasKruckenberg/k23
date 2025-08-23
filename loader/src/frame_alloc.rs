// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::range::Range;
use core::{cmp, iter, ptr, slice};

use fallible_iterator::FallibleIterator;

use crate::arch;
use crate::error::Error;

pub struct FrameAllocator<'a> {
    regions: &'a [Range<usize>],
    // offset from the top of memory regions
    offset: usize,
}

impl<'a> FrameAllocator<'a> {
    /// Create a new frame allocator over a given set of physical memory regions.
    #[must_use]
    pub fn new(regions: &'a [Range<usize>]) -> Self {
        Self { regions, offset: 0 }
    }

    #[must_use]
    pub fn free_regions(&self) -> FreeRegions<'_> {
        FreeRegions {
            offset: self.offset,
            inner: self.regions.iter().rev().copied(),
        }
    }

    #[must_use]
    pub fn used_regions(&self) -> UsedRegions<'_> {
        UsedRegions {
            offset: self.offset,
            inner: self.regions.iter().rev().copied(),
        }
    }

    pub fn frame_usage(&self) -> usize {
        self.offset >> arch::PAGE_SHIFT
    }

    pub fn allocate_one_zeroed(&mut self, phys_offset: usize) -> Result<usize, Error> {
        self.allocate_contiguous_zeroed(
            // Safety: the layout is always valid
            unsafe { Layout::from_size_align_unchecked(arch::PAGE_SIZE, arch::PAGE_SIZE) },
            phys_offset,
        )
    }

    pub fn allocate(&mut self, layout: Layout) -> FrameIter<'a, '_> {
        assert_eq!(
            layout.align(),
            arch::PAGE_SIZE,
            "BootstrapAllocator only supports page-aligned allocations"
        );

        let remaining = layout.pad_to_align().size();

        debug_assert!(remaining.is_multiple_of(arch::PAGE_SIZE));
        FrameIter {
            alloc: self,
            remaining,
        }
    }

    pub fn allocate_zeroed(
        &mut self,
        layout: Layout,
        phys_offset: usize,
    ) -> FrameIterZeroed<'a, '_> {
        FrameIterZeroed {
            inner: self.allocate(layout),
            phys_offset,
        }
    }

    pub fn allocate_contiguous(&mut self, layout: Layout) -> Result<usize, Error> {
        let requested_size = layout.pad_to_align().size();
        assert_eq!(
            layout.align(),
            arch::PAGE_SIZE,
            "BootstrapAllocator only supports page-aligned allocations"
        );
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.end.checked_sub(region.start).unwrap();

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

                let frame = region.end.checked_sub(offset + requested_size).unwrap();
                self.offset += requested_size;
                return Ok(frame);
            }

            offset -= region_size;
        }

        Err(Error::NoMemory)
    }

    pub fn allocate_contiguous_zeroed(
        &mut self,
        layout: Layout,
        phys_offset: usize,
    ) -> Result<usize, Error> {
        let requested_size = layout.pad_to_align().size();
        let addr = self.allocate_contiguous(layout)?;
        // Safety: we just allocated the frame
        unsafe {
            ptr::write_bytes::<u8>(
                phys_offset.checked_add(addr).unwrap() as *mut u8,
                0,
                requested_size,
            );
        }
        Ok(addr)
    }
}

pub struct FrameIter<'a, 'b> {
    alloc: &'b mut FrameAllocator<'a>,
    remaining: usize,
}

impl<'a> FrameIter<'a, '_> {
    pub fn alloc(&mut self) -> &mut FrameAllocator<'a> {
        self.alloc
    }
}

impl FallibleIterator for FrameIter<'_, '_> {
    type Item = (usize, NonZeroUsize);
    type Error = Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        if self.remaining > 0 {
            let mut offset = self.alloc.offset;

            for region in self.alloc.regions.iter().rev() {
                let region_size = region.end.checked_sub(region.start).unwrap();

                // only consider regions that we haven't already exhausted
                if let Some(allocatable_size) = region_size.checked_sub(offset)
                    && allocatable_size >= arch::PAGE_SIZE
                {
                    let allocation_size = cmp::min(self.remaining, allocatable_size)
                        & 0usize.wrapping_sub(arch::PAGE_SIZE);
                    debug_assert!(allocation_size.is_multiple_of(arch::PAGE_SIZE));

                    let frame = region.end.checked_sub(offset + allocation_size).unwrap();
                    self.alloc.offset += allocation_size;
                    self.remaining -= allocation_size;

                    return Ok(Some((frame, NonZeroUsize::new(allocation_size).unwrap())));
                }

                offset -= region_size;
            }

            Err(Error::NoMemory)
        } else {
            Ok(None)
        }
    }
}

pub struct FrameIterZeroed<'a, 'b> {
    inner: FrameIter<'a, 'b>,
    phys_offset: usize,
}

impl<'a> FrameIterZeroed<'a, '_> {
    pub fn alloc(&mut self) -> &mut FrameAllocator<'a> {
        self.inner.alloc
    }
}

impl FallibleIterator for FrameIterZeroed<'_, '_> {
    type Item = (usize, NonZeroUsize);
    type Error = Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let Some((base, len)) = self.inner.next()? else {
            return Ok(None);
        };

        // Safety: we just allocated the frame
        unsafe {
            ptr::write_bytes::<u8>(
                self.phys_offset.checked_add(base).unwrap() as *mut u8,
                0,
                len.get(),
            );
        }

        Ok(Some((base, len)))
    }
}

pub struct FreeRegions<'a> {
    offset: usize,
    inner: iter::Copied<iter::Rev<slice::Iter<'a, Range<usize>>>>,
}

impl Iterator for FreeRegions<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut region = self.inner.next()?;
            // keep advancing past already fully used memory regions
            let region_size = region.end.checked_sub(region.start).unwrap();
            if self.offset >= region_size {
                self.offset -= region_size;
                continue;
            } else if self.offset > 0 {
                region.end = region.end.checked_sub(self.offset).unwrap();
                self.offset = 0;
            }

            return Some(region);
        }
    }
}

pub struct UsedRegions<'a> {
    offset: usize,
    inner: iter::Copied<iter::Rev<slice::Iter<'a, Range<usize>>>>,
}

impl Iterator for UsedRegions<'_> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut region = self.inner.next()?;

        if self.offset >= region.end.checked_sub(region.start).unwrap() {
            Some(region)
        } else if self.offset > 0 {
            region.start = region.end.checked_sub(self.offset).unwrap();
            self.offset = 0;

            Some(region)
        } else {
            None
        }
    }
}
