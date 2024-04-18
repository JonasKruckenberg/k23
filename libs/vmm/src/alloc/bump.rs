use crate::{
    zero_frames, AddressRangeExt, Error, FrameAllocator, FrameUsage, Mode, PhysicalAddress,
};
use core::marker::PhantomData;
use core::ops::Range;

#[derive(Debug)]
pub struct BumpAllocator<'a, M> {
    regions: &'a [Range<PhysicalAddress>],
    // offset from the top of memory regions
    offset: usize,
    _m: PhantomData<M>,
}

impl<'a, M: Mode> BumpAllocator<'a, M> {
    /// Create a new frame allocator over a given set of physical memory regions.
    ///
    /// # Safety
    ///
    /// The caller has to ensure the slice is correctly sorted from lowest to highest addresses.
    pub unsafe fn new(regions: &'a [Range<PhysicalAddress>], offset: usize) -> Self {
        Self {
            regions,
            offset,
            _m: PhantomData,
        }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn regions(&self) -> &'a [Range<PhysicalAddress>] {
        self.regions
    }
}

impl<'a, M: Mode> FrameAllocator for BumpAllocator<'a, M> {
    fn allocate_frames(&mut self, frames: usize) -> crate::Result<PhysicalAddress> {
        let requested_size = frames * M::PAGE_SIZE;
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.size();
            if offset < region_size {
                if region_size - offset < requested_size {
                    log::warn!("Skipped memory region {region:?} since it was fullfill request for {requested_size} bytes. Wasted {region_size} bytes in the process...");

                    self.offset += region_size - offset;
                    offset = 0;
                    continue;
                }

                let page_phys = region.end.sub(offset + requested_size);
                let page_virt = M::phys_to_virt(page_phys);
                zero_frames::<M>(page_virt.as_raw() as *mut u64, frames);

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
        for region in self.regions.iter() {
            let region_size = region.end.0 - region.start.0;
            total += region_size >> M::PAGE_SHIFT;
        }
        let used = self.offset >> M::PAGE_SHIFT;
        FrameUsage { used, total }
    }
}

impl<'a, M> BumpAllocator<'a, crate::INIT<M>> {
    pub fn end_init(self) -> BumpAllocator<'a, M> {
        BumpAllocator {
            regions: self.regions,
            offset: self.offset,
            _m: PhantomData,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{BumpAllocator, EmulateArch, Error, FrameAllocator, Mode, PhysicalAddress};

    #[test]
    fn single_region_single_frame() -> Result<(), Error> {
        let mut alloc: BumpAllocator<EmulateArch> = unsafe {
            BumpAllocator::new(&[PhysicalAddress(0)..PhysicalAddress(4 * EmulateArch::PAGE_SIZE)])
        };

        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x3000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x2000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x1000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));

        Ok(())
    }

    #[test]
    fn single_region_multi_frame() -> Result<(), Error> {
        let mut alloc: BumpAllocator<EmulateArch> = unsafe {
            BumpAllocator::new(&[PhysicalAddress(0)..PhysicalAddress(4 * EmulateArch::PAGE_SIZE)])
        };

        assert_eq!(alloc.allocate_frames(3)?, PhysicalAddress(0x1000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));

        Ok(())
    }

    #[test]
    fn multi_region_single_frame() -> Result<(), Error> {
        let mut alloc: BumpAllocator<EmulateArch> = unsafe {
            BumpAllocator::new(&[
                PhysicalAddress(0)..PhysicalAddress(4 * EmulateArch::PAGE_SIZE),
                PhysicalAddress(7 * EmulateArch::PAGE_SIZE)
                    ..PhysicalAddress(9 * EmulateArch::PAGE_SIZE),
            ])
        };

        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x8000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x7000));

        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x3000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x2000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x1000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));

        Ok(())
    }

    #[test]
    fn multi_region_multi_frame() -> Result<(), Error> {
        let mut alloc: BumpAllocator<EmulateArch> = unsafe {
            BumpAllocator::new(&[
                PhysicalAddress(0)..PhysicalAddress(4 * EmulateArch::PAGE_SIZE),
                PhysicalAddress(7 * EmulateArch::PAGE_SIZE)
                    ..PhysicalAddress(9 * EmulateArch::PAGE_SIZE),
            ])
        };

        assert_eq!(alloc.allocate_frames(2)?, PhysicalAddress(0x7000));

        assert_eq!(alloc.allocate_frames(2)?, PhysicalAddress(0x2000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x1000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));

        Ok(())
    }

    #[test]
    fn multi_region_multi_frame2() -> Result<(), Error> {
        let mut alloc: BumpAllocator<EmulateArch> = unsafe {
            BumpAllocator::new(&[
                PhysicalAddress(0)..PhysicalAddress(4 * EmulateArch::PAGE_SIZE),
                PhysicalAddress(7 * EmulateArch::PAGE_SIZE)
                    ..PhysicalAddress(9 * EmulateArch::PAGE_SIZE),
            ])
        };

        assert_eq!(alloc.allocate_frames(3)?, PhysicalAddress(0x1000));
        assert_eq!(alloc.allocate_frames(1)?, PhysicalAddress(0x0));
        assert!(matches!(alloc.allocate_frames(1), Err(Error::OutOfMemory)));

        Ok(())
    }
}
