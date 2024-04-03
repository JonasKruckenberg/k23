use crate::alloc::{FrameAllocator, FrameUsage};
use crate::{AddressRangeExt, Error};
use crate::{Mode, PhysicalAddress};
use core::marker::PhantomData;
use core::ops::Range;

pub struct BumpAllocator<M> {
    region: Range<PhysicalAddress>,
    offset: usize,
    _m: PhantomData<M>,
}

impl<M: Mode> BumpAllocator<M> {
    /// # Safety
    ///
    /// The regions list is assumed to be sorted and not overlapping
    pub unsafe fn new(region: Range<PhysicalAddress>, offset: usize) -> Self {
        Self {
            region,
            offset,
            _m: PhantomData,
        }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn region(&self) -> &Range<PhysicalAddress> {
        &self.region
    }
}

impl<M: Mode> FrameAllocator<M> for BumpAllocator<M> {
    fn allocate_frames(&mut self, num_frames: usize) -> crate::Result<PhysicalAddress> {
        let new_offset = self.offset + num_frames * M::PAGE_SIZE;

        if new_offset <= self.region.size() {
            let page_phys = self.region.start.add(self.offset);
            self.offset += num_frames * M::PAGE_SIZE;

            Ok(page_phys)
        } else {
            Err(Error::OutOfMemory)
        }
    }

    fn deallocate_frames(&mut self, _: PhysicalAddress, _: usize) -> crate::Result<()> {
        unimplemented!("BumpAllocator can't free")
    }

    fn frame_usage(&self) -> FrameUsage {
        let total = self.offset();
        let used = self.offset >> M::PAGE_SHIFT;
        FrameUsage { used, total }
    }
}

#[cfg(target_arch = "riscv64")]
impl<M> BumpAllocator<crate::INIT<M>> {
    pub fn consume_init(self) -> BumpAllocator<M> {
        BumpAllocator {
            region: self.region,
            offset: self.offset,
            _m: PhantomData,
        }
    }
}

// pub struct BumpAllocator<'a, M> {
//     regions: &'a [Range<PhysicalAddress>],
//     offset: usize,
//     _m: PhantomData<M>,
// }
//
// impl<'a, M: Mode> BumpAllocator<'a, M> {
//     /// # Safety
//     ///
//     /// The regions list is assumed to be sorted and not overlapping
//     pub unsafe fn new(regions: &'a [Range<PhysicalAddress>], offset: usize) -> Self {
//         Self {
//             regions,
//             offset,
//             _m: PhantomData,
//         }
//     }
//
//     pub fn offset(&self) -> usize {
//         self.offset
//     }
//
//     pub fn regions(&self) -> &'a [Range<PhysicalAddress>] {
//         self.regions
//     }
// }
//
// impl<'a, M: Mode> FrameAllocator<M> for BumpAllocator<'a, M> {
//     fn allocate_frames(&mut self, num_frames: usize) -> crate::Result<PhysicalAddress> {
//         let mut offset = self.offset + num_frames * M::PAGE_SIZE;
//
//         for region in self.regions.iter() {
//             let region_size = region.end.0 - region.start.0;
//
//             if offset < region_size {
//                 let page_phys = region.start.add(offset);
//                 // let page_virt = (self.phys_to_virt)(page_phys);
//                 // zero_frames::<M>(page_virt.0 as *mut u64, num_frames);
//                 self.offset += num_frames * M::PAGE_SIZE;
//                 return Ok(page_phys);
//             }
//             offset -= region_size;
//         }
//
//         Err(Error::OutOfMemory)
//     }
//
//     fn deallocate_frames(&mut self, _: PhysicalAddress, _: usize) -> crate::Result<()> {
//         unimplemented!("BumpAllocator can't free")
//     }
//
//     fn frame_usage(&self) -> FrameUsage {
//         let mut total = 0;
//         for region in self.regions.iter() {
//             let region_size = region.end.0 - region.start.0;
//             total += region_size >> M::PAGE_SHIFT;
//         }
//         let used = self.offset >> M::PAGE_SHIFT;
//         FrameUsage { used, total }
//     }
// }
//
// #[cfg(target_arch = "riscv64")]
// impl<'a, M> BumpAllocator<'a, crate::INIT<M>> {
//     pub fn consume_init(self) -> BumpAllocator<'a, M> {
//         BumpAllocator {
//             regions: self.regions,
//             offset: self.offset,
//             _m: PhantomData,
//         }
//     }
// }
