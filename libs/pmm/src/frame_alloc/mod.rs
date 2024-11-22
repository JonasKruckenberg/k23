pub mod bitmap;
pub mod bump;

use crate::{Arch, PhysicalAddress, VirtualAddress};

#[derive(Debug)]
pub struct FrameUsage {
    pub used: usize,
    pub total: usize,
}

pub trait FrameAllocator<A>
where
    A: Arch,
{
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

    /// Converts a physical address to a virtual address
    fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress;

    /// Allocates a frame and zeroes it.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be allocated.
    fn allocate_frame_zeroed(&mut self) -> crate::Result<PhysicalAddress> {
        let page_phys = self.allocate_frames(1)?;
        let page_virt = self.phys_to_virt(page_phys);
        zero_frames(page_virt.as_raw() as *mut u64, A::PAGE_SIZE);
        Ok(page_phys)
    }

    /// Allocates a number of frames and zero them.
    ///
    /// # Errors
    ///
    /// Returns an error if the frames cannot be allocated.
    fn allocate_frames_zeroed(&mut self, frames: usize) -> crate::Result<PhysicalAddress> {
        let page_phys = self.allocate_frames(frames)?;
        let page_virt = self.phys_to_virt(page_phys);
        zero_frames(page_virt.as_raw() as *mut u64, frames * A::PAGE_SIZE);
        Ok(page_phys)
    }
}

pub(crate) fn zero_frames(mut ptr: *mut u64, bytes: usize) {
    unsafe {
        let end = ptr.add(bytes / size_of::<u64>());
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }
}
