use crate::frame_alloc::FrameAllocatorImpl;
use crate::{AddressRangeExt, Arch, Error, FrameUsage, PhysicalAddress};
use core::marker::PhantomData;
use core::ops::Range;
use core::{cmp, iter, slice};

pub struct BumpAllocator<'a, A> {
    regions: &'a [Range<PhysicalAddress>],
    // offset from the top of memory regions
    offset: usize,
    lower_bound: PhysicalAddress,
    _m: PhantomData<A>,
}

impl<'a, A> BumpAllocator<'a, A> {
    /// Create a new frame allocator over a given set of physical memory regions.
    ///
    /// # Safety
    ///
    /// The caller has to ensure the slice is correctly sorted from lowest to highest addresses.
    #[must_use]
    pub unsafe fn new(regions: &'a [Range<PhysicalAddress>]) -> Self {
        Self {
            regions,
            offset: 0,
            lower_bound: PhysicalAddress(0),
            _m: PhantomData,
        }
    }

    /// Create a new frame allocator over a given set of physical memory regions.
    ///
    /// # Safety
    ///
    /// The caller has to ensure the slice is correctly sorted from lowest to highest addresses.
    #[must_use]
    pub unsafe fn new_with_lower_bound(
        regions: &'a [Range<PhysicalAddress>],
        lower_bound: PhysicalAddress,
    ) -> Self {
        Self {
            regions,
            offset: 0,
            lower_bound,
            _m: PhantomData,
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

impl<'a, A> FrameAllocatorImpl<A> for BumpAllocator<'a, A>
where
    A: Arch,
{
    fn allocate_non_contiguous(&mut self, count: usize) -> crate::Result<Range<PhysicalAddress>> {
        let requested_size = count * A::PAGE_SIZE;
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            // only consider regions that we haven't already exhausted
            if offset < region.size() {
                let alloc_size = cmp::min(requested_size, region.size() - offset);

                let page_phys = region.end.sub(offset + alloc_size);

                if page_phys <= self.lower_bound {
                    log::error!(
                        "Allocation would have crossed `lower_bound`: {} <= {}",
                        page_phys,
                        self.lower_bound
                    );
                    return Err(Error::OutOfMemory);
                }

                self.offset += alloc_size;

                return Ok(page_phys..page_phys.add(alloc_size));
            }

            offset -= region.size();
        }

        Err(Error::OutOfMemory)
    }

    fn allocate_contiguous(&mut self, count: usize) -> crate::Result<PhysicalAddress> {
        let requested_size = count * A::PAGE_SIZE;
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.size();
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

                let page_phys = region.end.sub(offset + requested_size);

                if page_phys <= self.lower_bound {
                    log::error!(
                        "Allocation would have crossed `lower_bound`: {} <= {}",
                        page_phys,
                        self.lower_bound
                    );
                    return Err(Error::OutOfMemory);
                }

                self.offset += requested_size;

                return Ok(page_phys);
            }

            offset -= region_size;
        }

        Err(Error::OutOfMemory)
    }

    fn frame_usage(&self) -> FrameUsage {
        let mut total = 0;
        for region in self.regions {
            let region_size = region.end.0 - region.start.0;
            total += region_size >> A::PAGE_SHIFT;
        }
        let used = self.offset >> A::PAGE_SHIFT;
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
            let region_size = region.size();
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
        let region_size = region.size();

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
