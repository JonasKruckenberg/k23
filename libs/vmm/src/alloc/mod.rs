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
    /// Allocates a frame.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be allocated.
    fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress> {
        self.allocate_frames(1)
    }
    /// Allocates a number of frames.
    ///
    /// # Errors
    ///
    /// Returns an error if the frames cannot be allocated.
    fn allocate_frames(&mut self, frames: usize) -> crate::Result<PhysicalAddress>;
    /// Deallocates a frame.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be deallocated.
    fn deallocate_frame(&mut self, base: PhysicalAddress) -> crate::Result<()> {
        self.deallocate_frames(base, 1)
    }
    /// Deallocates a number of frames.
    ///
    /// # Errors
    ///
    /// Returns an error if the frames cannot be deallocated.
    fn deallocate_frames(&mut self, base: PhysicalAddress, frames: usize) -> crate::Result<()>;

    /// Information about the number of physical frames used, and available
    fn frame_usage(&self) -> FrameUsage;

    /// Allocates a frame and zeroes it.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be allocated.
    fn allocate_frame_zeroed(&mut self) -> crate::Result<PhysicalAddress> {
        let page_phys = self.allocate_frames(1)?;
        let page_virt = M::phys_to_virt(page_phys);
        zero_frames::<M>(page_virt.as_raw() as *mut u64, 1);
        Ok(page_phys)
    }
    /// Allocates a number of frames and zero them.
    ///
    /// # Errors
    ///
    /// Returns an error if the frames cannot be allocated.
    fn allocate_frames_zeroed(&mut self, frames: usize) -> crate::Result<PhysicalAddress> {
        let page_phys = self.allocate_frames(frames)?;
        let page_virt = M::phys_to_virt(page_phys);
        zero_frames::<M>(page_virt.as_raw() as *mut u64, frames);
        Ok(page_phys)
    }
}
