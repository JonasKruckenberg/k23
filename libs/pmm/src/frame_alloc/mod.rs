mod bitmap;
mod bump;

use crate::arch::PAGE_SIZE;
use crate::{Error, PhysicalAddress, VirtualAddress};
use core::num::NonZeroUsize;

pub use bitmap::BitMapAllocator;
pub use bump::{BumpAllocator, FreeRegions, UsedRegions};

#[derive(Debug)]
pub struct FrameUsage {
    pub used: usize,
    pub total: usize,
}

pub trait FrameAllocator {
    fn allocate_contiguous(
        &mut self,
        frames: NonZeroUsize,
    ) -> crate::Result<(PhysicalAddress, NonZeroUsize)>;
    fn deallocate(&mut self, base: PhysicalAddress, frames: NonZeroUsize) -> crate::Result<()>;

    fn allocate_contiguous_all(&mut self, frames: NonZeroUsize) -> crate::Result<PhysicalAddress> {
        let (phys, allocated) = self.allocate_contiguous(frames)?;
        if allocated != frames {
            Err(Error::OutOfMemory)
        } else {
            Ok(phys)
        }
    }

    fn allocate_one(&mut self) -> crate::Result<PhysicalAddress> {
        self.allocate_contiguous_all(NonZeroUsize::new(1).unwrap())
    }

    fn allocate_one_zeroed(
        &mut self,
        phys_offset: VirtualAddress,
    ) -> crate::Result<PhysicalAddress> {
        let frame = NonZeroUsize::new(1).unwrap();
        let phys = self.allocate_contiguous_all(frame)?;
        let virt = VirtualAddress::from_phys(phys, phys_offset);
        unsafe {
            zero_pages(virt.as_raw() as _, frame);
        }
        Ok(phys)
    }

    fn allocate_contiguous_zeroed(
        &mut self,
        frames: NonZeroUsize,
        phys_offset: VirtualAddress,
    ) -> crate::Result<(PhysicalAddress, NonZeroUsize)> {
        let (phys, frames) = self.allocate_contiguous(frames)?;
        let virt = VirtualAddress::from_phys(phys, phys_offset);
        unsafe {
            zero_pages(virt.as_raw() as _, frames);
        }
        Ok((phys, frames))
    }

    fn allocate_contiguous_all_zeroed(
        &mut self,
        frames: NonZeroUsize,
        phys_offset: VirtualAddress,
    ) -> crate::Result<PhysicalAddress> {
        let phys = self.allocate_contiguous_all(frames)?;
        let virt = VirtualAddress::from_phys(phys, phys_offset);
        unsafe {
            zero_pages(virt.as_raw() as _, frames);
        }
        Ok(phys)
    }

    fn deallocate_one(&mut self, base: PhysicalAddress) -> crate::Result<()> {
        self.deallocate(base, NonZeroUsize::new(1).unwrap())
    }

    /// Information about the number of physical frames used and available
    fn frame_usage(&self) -> FrameUsage;
}

pub struct NonContiguousFrames<'a> {
    alloc: &'a mut dyn FrameAllocator,
    remaining: usize,
    zeroed: Option<VirtualAddress>,
}

impl<'a> NonContiguousFrames<'a> {
    pub fn new(alloc: &'a mut dyn FrameAllocator, frames: NonZeroUsize) -> Self {
        Self {
            alloc,
            remaining: frames.get(),
            zeroed: None,
        }
    }
    pub fn new_zeroed(
        alloc: &'a mut dyn FrameAllocator,
        frames: NonZeroUsize,
        phys_offset: VirtualAddress,
    ) -> Self {
        Self {
            alloc,
            remaining: frames.get(),
            zeroed: Some(phys_offset),
        }
    }
    pub fn alloc_mut(&mut self) -> &mut dyn FrameAllocator {
        self.alloc
    }
}
impl Iterator for NonContiguousFrames<'_> {
    type Item = crate::Result<(PhysicalAddress, NonZeroUsize)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let res = self
            .alloc
            .allocate_contiguous(NonZeroUsize::new(self.remaining).unwrap());

        match res {
            Ok((phys, frames)) => {
                self.remaining -= frames.get();

                if let Some(phys_offset) = self.zeroed {
                    let virt = VirtualAddress::from_phys(phys, phys_offset);
                    unsafe {
                        zero_pages(virt.as_raw() as _, frames);
                    }
                }
            }
            Err(_) => {
                self.remaining = 0;
            }
        }

        Some(res)
    }
}

/// Fill a given number of pages with zeroes.
///
/// # Safety
///
/// The caller has to ensure the entire range is valid and accessible.
pub unsafe fn zero_pages(mut ptr: *mut u64, num_pages: NonZeroUsize) {
    unsafe {
        let end = ptr.add((num_pages.get() * PAGE_SIZE) / size_of::<u64>());
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }
}
