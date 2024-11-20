use crate::{
    AddressRangeExt, Error, FrameAllocator, FrameUsage, Mode, PhysicalAddress, VirtualAddress,
};
use core::marker::PhantomData;
use core::ops::Range;
use core::{iter, slice};

pub struct BumpAllocator<'a, M> {
    regions: &'a [Range<PhysicalAddress>],
    // offset from the top of memory regions
    offset: usize,
    lower_bound: PhysicalAddress,
    pub(crate) physmem_off: VirtualAddress,
    _m: PhantomData<M>,
}

impl<'a, M: Mode> BumpAllocator<'a, M> {
    /// Create a new frame allocator over a given set of physical memory regions.
    ///
    /// # Safety
    ///
    /// The caller has to ensure the slice is correctly sorted from lowest to highest addresses.
    #[must_use]
    pub unsafe fn new(regions: &'a [Range<PhysicalAddress>], physmem_off: VirtualAddress) -> Self {
        Self {
            regions,
            offset: 0,
            lower_bound: PhysicalAddress(0),
            physmem_off,
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
        physmem_off: VirtualAddress,
    ) -> Self {
        Self {
            regions,
            offset: 0,
            lower_bound,
            physmem_off,
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

impl<M: Mode> FrameAllocator<M> for BumpAllocator<'_, M> {
    fn allocate_frames(&mut self, frames: usize) -> crate::Result<PhysicalAddress> {
        let requested_size = frames * M::PAGE_SIZE;
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.size();
            if offset < region_size {
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

    fn deallocate_frames(&mut self, _base: PhysicalAddress, _frames: usize) -> crate::Result<()> {
        unimplemented!("BumpAllocator can't free")
    }

    fn frame_usage(&self) -> FrameUsage {
        let mut total = 0;
        for region in self.regions {
            let region_size = region.end.0 - region.start.0;
            total += region_size >> M::PAGE_SHIFT;
        }
        let used = self.offset >> M::PAGE_SHIFT;
        FrameUsage { used, total }
    }

    fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        self.physmem_off.add(phys.as_raw())
    }
}

#[cfg(test)]
mod test {
    use crate::{
        BumpAllocator, EmulateMode, Error, FrameAllocator, Mode, PhysicalAddress, VirtualAddress,
    };
    use ktest::test;

    #[test]
    fn single_region_single_frame() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x4000)..top];
        let mut alloc: BumpAllocator<EmulateMode> =
            unsafe { BumpAllocator::new(&regions, VirtualAddress::default()) };

        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x1000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x2000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x3000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x4000));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
    }

    #[test]
    fn single_region_multi_frame() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x4000)..top];
        let mut alloc: BumpAllocator<EmulateMode> =
            unsafe { BumpAllocator::new(&regions, VirtualAddress::default()) };

        assert_eq!(alloc.allocate_frames(3).unwrap(), top.sub(0x3000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x4000));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
    }

    #[test]
    fn multi_region_single_frame() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x4000)..top, top.sub(0x9000)..top.sub(0x7000)];
        let mut alloc: BumpAllocator<EmulateMode> =
            unsafe { BumpAllocator::new(&regions, VirtualAddress::default()) };

        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x8000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x9000));

        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x1000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x2000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x3000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x4000));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
    }

    #[test]
    fn multi_region_multi_frame() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x4000)..top, top.sub(0x9000)..top.sub(0x7000)];
        let mut alloc: BumpAllocator<EmulateMode> =
            unsafe { BumpAllocator::new(&regions, VirtualAddress::default()) };

        assert_eq!(alloc.allocate_frames(2).unwrap(), top.sub(0x9000));

        assert_eq!(alloc.allocate_frames(2).unwrap(), top.sub(0x2000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x3000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x4000));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
    }

    #[test]
    fn multi_region_multi_frame2() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x4000)..top, top.sub(0x9000)..top.sub(0x7000)];
        let mut alloc: BumpAllocator<EmulateMode> =
            unsafe { BumpAllocator::new(&regions, VirtualAddress::default()) };

        assert_eq!(alloc.allocate_frames(3).unwrap(), top.sub(0x3000));
        assert_eq!(alloc.allocate_frames(1).unwrap(), top.sub(0x4000));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));
    }
}
