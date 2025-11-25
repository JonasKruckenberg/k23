use core::ops::Range;
use crate::{PhysicalAddress, VirtualAddress};

pub struct HardwareAddressSpace {}

impl HardwareAddressSpace {
    pub unsafe fn map_phys<F>(
        &mut self,
        mut virt: Range<VirtualAddress>,
        mut phys: impl FallibleIterator<Item = Range<PhysicalAddress>, Error = AllocError>,
        attributes: MemoryAttributes,
        frame_allocator: F,
        flush: &mut Flush,
    ) -> Result<(), AllocError>
    where
        F: FrameAllocator,
    {
}