use crate::arch::PAGE_SIZE;
use crate::frame_alloc::FrameUsage;
use crate::{arch, frame_alloc::FrameAllocator, Error, PhysicalAddress};
use core::num::NonZeroUsize;
use core::ops::Range;
use core::{cmp, iter, slice};

pub struct BumpAllocator<'a> {
    regions: &'a [Range<PhysicalAddress>],
    // offset from the top of memory regions
    offset: usize,
    lower_bound: PhysicalAddress,
}
impl<'a> BumpAllocator<'a> {
    /// Create a new frame allocator over a given set of physical memory regions.
    #[must_use]
    pub fn new(regions: &'a [Range<PhysicalAddress>]) -> Self {
        Self {
            regions,
            offset: 0,
            lower_bound: PhysicalAddress(0),
        }
    }

    /// Create a new frame allocator over a given set of physical memory regions.
    #[must_use]
    pub fn new_with_lower_bound(
        regions: &'a [Range<PhysicalAddress>],
        lower_bound: PhysicalAddress,
    ) -> Self {
        Self {
            regions,
            offset: 0,
            lower_bound,
        }
    }

    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    #[must_use]
    pub fn regions(&self) -> &'a [Range<PhysicalAddress>] {
        self.regions
    }

    #[must_use]
    pub fn free_regions(&self) -> FreeRegions<'_> {
        FreeRegions {
            offset: self.offset,
            inner: self.regions().iter().rev().cloned(),
        }
    }

    #[must_use]
    pub fn used_regions(&self) -> UsedRegions<'_> {
        UsedRegions {
            offset: self.offset,
            inner: self.regions().iter().rev().cloned(),
        }
    }
}

impl FrameAllocator for BumpAllocator<'_> {
    fn allocate_contiguous(
        &mut self,
        frames: NonZeroUsize,
    ) -> crate::Result<(PhysicalAddress, NonZeroUsize)> {
        let requested_size = frames.get() * PAGE_SIZE;
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.end.as_raw() - region.start.as_raw();

            // only consider regions that we haven't already exhausted
            if offset < region_size {
                let alloc_size = cmp::min(requested_size, region_size - offset);

                let frame = region.end.sub(offset + alloc_size);
                if frame <= self.lower_bound {
                    log::error!(
                        "Allocation would have crossed `lower_bound`: {} <= {}",
                        frame,
                        self.lower_bound
                    );
                    return Err(Error::OutOfMemory);
                }
                self.offset += alloc_size;

                return Ok((frame, NonZeroUsize::new(alloc_size / PAGE_SIZE).unwrap()));
            }

            offset -= region_size;
        }

        Err(Error::OutOfMemory)
    }

    fn deallocate(&mut self, _base: PhysicalAddress, _frames: NonZeroUsize) -> crate::Result<()> {
        unimplemented!("Bump allocator can't free");
    }

    fn allocate_contiguous_all(&mut self, frames: NonZeroUsize) -> crate::Result<PhysicalAddress> {
        let requested_size = frames.get() * PAGE_SIZE;
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.end.as_raw() - region.start.as_raw();

            // only consider regions that we haven't already exhausted
            if offset < region_size {
                // Allocating a contiguous range has different requirements than "regular" allocation
                // contiguous are rare and often happen in very critical paths where e.g. virtual
                // memory is not available yet. So we rather waste some memory than outright crash.
                if region_size - offset < requested_size {
                    log::warn!("Skipped memory region {region:?} since it was fulfill request for {requested_size} bytes. Wasted {} bytes in the process...", region_size - offset);

                    self.offset += region_size - offset;
                    offset = 0;
                    continue;
                }

                let frame = region.end.sub(offset + requested_size);

                if frame <= self.lower_bound {
                    log::error!(
                        "Allocation would have crossed `lower_bound`: {} <= {}",
                        frame,
                        self.lower_bound
                    );
                    return Err(Error::OutOfMemory);
                }

                self.offset += requested_size;

                return Ok(frame);
            }

            offset -= region_size;
        }

        Err(Error::OutOfMemory)
    }

    fn frame_usage(&self) -> FrameUsage {
        let mut total = 0;
        for region in self.regions {
            let region_size = region.end.0 - region.start.0;
            total += region_size >> arch::PAGE_SHIFT;
        }
        let used = self.offset >> arch::PAGE_SHIFT;
        FrameUsage { used, total }
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
            let region_size = region.end.as_raw() - region.start.as_raw();
            // keep advancing past already fully used memory regions
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
        let region_size = region.end.as_raw() - region.start.as_raw();

        if self.offset >= region_size {
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
