use core::ops::RangeBounds;

use kmem_core::PhysicalAddress;

pub trait Vmo {
    // Acquire the frame at the given `index`. After this call succeeds, all accessed following the
    // given `access_rules` MUST NOT fault.
    // UNIT: frames
    fn acquire(
        &self,
        range: impl RangeBounds<usize>,
    ) -> Result<impl Iterator<Item = PhysicalAddress>, ()>;

    // Release the frame at the given `index`. After this call succeeds, all accessed to the frame
    // MUST fault. Returns the base physical address of the released frames.
    // UNIT: frames
    fn release(
        &self,
        range: impl RangeBounds<usize>,
    ) -> Result<impl Iterator<Item = PhysicalAddress>, ()>;

    fn clear(
        &self,
        range: impl RangeBounds<usize>,
    ) -> Result<impl Iterator<Item = PhysicalAddress>, ()>;
}
