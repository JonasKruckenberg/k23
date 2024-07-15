mod bitmap;
mod bump;

use crate::{zero_frames, PhysicalAddress};

pub use bitmap::BitMapAllocator;
pub use bump::BumpAllocator;

#[derive(Debug)]
pub struct FrameUsage {
    pub used: usize,
    pub total: usize,
}

pub trait FrameAllocator<M: crate::Mode> {
    fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress> {
        self.allocate_frames(1)
    }
    fn allocate_frames(&mut self, frames: usize) -> crate::Result<PhysicalAddress>;
    fn deallocate_frame(&mut self, base: PhysicalAddress) -> crate::Result<()> {
        self.deallocate_frames(base, 1)
    }
    fn deallocate_frames(&mut self, base: PhysicalAddress, frames: usize) -> crate::Result<()>;

    /// Information about the number of physical frames used, and available
    fn frame_usage(&self) -> FrameUsage;

    fn allocate_frame_zeroed(&mut self) -> crate::Result<PhysicalAddress> {
        let page_phys = self.allocate_frames(1)?;
        let page_virt = M::phys_to_virt(page_phys);
        zero_frames::<M>(page_virt.as_raw() as *mut u64, 1);
        Ok(page_phys)
    }
    fn allocate_frames_zeroed(&mut self, frames: usize) -> crate::Result<PhysicalAddress> {
        let page_phys = self.allocate_frames(frames)?;
        let page_virt = M::phys_to_virt(page_phys);
        zero_frames::<M>(page_virt.as_raw() as *mut u64, frames);
        Ok(page_phys)
    }
}
