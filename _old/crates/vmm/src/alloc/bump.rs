use crate::alloc::{zero_frames, FrameAllocator, FrameUsage};
use crate::{Error, VirtualAddress};
use crate::{Mode, PhysicalAddress};
use core::marker::PhantomData;
use core::ops::Range;

pub struct BumpAllocator<'a, M> {
    regions: &'a [Range<PhysicalAddress>],
    offset: usize,
    phys_to_virt: fn(PhysicalAddress) -> VirtualAddress,
    _m: PhantomData<M>,
}

impl<'a, M: Mode> BumpAllocator<'a, M> {
    /// # Safety
    ///
    /// The regions list is assumed to be sorted and not overlapping
    pub unsafe fn new(
        regions: &'a [Range<PhysicalAddress>],
        offset: usize,
        phys_to_virt: fn(PhysicalAddress) -> VirtualAddress,
    ) -> Self {
        Self {
            regions,
            offset,
            phys_to_virt,
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

impl<'a, M: Mode> FrameAllocator<M> for BumpAllocator<'a, M> {
    fn allocate_frames(&mut self, num_frames: usize) -> crate::Result<PhysicalAddress> {
        let mut offset = self.offset + num_frames * M::PAGE_SIZE;

        for region in self.regions.iter() {
            let region_size = region.end.0 - region.start.0;

            if offset < region_size {
                let page_phys = region.start.add(offset);
                let page_virt = (self.phys_to_virt)(page_phys);
                zero_frames::<M>(page_virt.0 as *mut u64, num_frames);
                self.offset += num_frames * M::PAGE_SIZE;
                return Ok(page_phys);
            }
            offset -= region_size;
        }

        Err(Error::OutOfMemory)
    }

    fn deallocate_frames(&mut self, _: PhysicalAddress, _: usize) -> crate::Result<()> {
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
