mod bump;
pub use bump::{BumpAllocator, FreeRegions, UsedRegions};

use crate::{AddressRangeExt, Arch, PhysicalAddress};
use core::marker::PhantomData;
use core::ops::Range;

#[derive(Debug)]
pub struct FrameUsage {
    pub used: usize,
    pub total: usize,
}

pub(self) trait FrameAllocatorImpl<A> {
    fn allocate_non_contiguous(&mut self, count: usize) -> crate::Result<Range<PhysicalAddress>>;
    fn allocate_contiguous(&mut self, count: usize) -> crate::Result<PhysicalAddress>;
    /// Return information about the number of physical frames used, and available
    fn frame_usage(&self) -> FrameUsage;
}

#[allow(private_bounds)]
pub trait FrameAllocator<A>: FrameAllocatorImpl<A> {
    fn allocate_frames(&mut self, count: usize) -> FramesIter<'_, Self, A>
    where
        Self: Sized;
    fn allocate_frames_contiguous(&mut self, count: usize) -> crate::Result<PhysicalAddress>;
    fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress>;
    fn frame_usage(&self) -> FrameUsage;
    
}
impl<A, T> FrameAllocator<A> for T
where
    T: FrameAllocatorImpl<A>,
    A: Arch
{
    fn allocate_frames(&mut self, count: usize) -> FramesIter<'_, Self, A>
    where
        Self: Sized,
    {
        FramesIter {
            alloc: self,
            rem_count: count,
            _m: PhantomData,
        }
    }

    fn allocate_frames_contiguous(&mut self, count: usize) -> crate::Result<PhysicalAddress> {
        let frame = self.allocate_contiguous(count)?;
        log::trace!("allocated {frame:?}..{:?}", frame.add(count * A::PAGE_SIZE));
        Ok(frame)
    }

    fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress> {
        // A single frame is always contiguous by definition, so this will only
        // fail if we're truly fully out of memory
        let frame = self.allocate_contiguous(1)?;
        log::trace!("allocated {frame:?}..{:?}", frame.add(A::PAGE_SIZE));
        Ok(frame)
    }

    fn frame_usage(&self) -> FrameUsage {
        FrameAllocatorImpl::frame_usage(self)
    }
}

pub struct FramesIter<'f, F, A> {
    alloc: &'f mut F,
    rem_count: usize,
    _m: PhantomData<A>,
}
impl<'f, F, A> FramesIter<'f, F, A> {
    pub fn alloc_mut(&mut self) -> &mut F {
        self.alloc
    }
}
impl<'f, F, A> Iterator for FramesIter<'f, F, A>
where
    A: Arch,
    F: FrameAllocator<A>,
{
    type Item = crate::Result<Range<PhysicalAddress>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rem_count == 0 {
            return None;
        }

        match self.alloc.allocate_non_contiguous(self.rem_count) {
            Ok(range) => {
                log::trace!("allocated {range:?}");
                self.rem_count -= range.size() / A::PAGE_SIZE;
                Some(Ok(range))
            }
            Err(err) => {
                self.rem_count = 0;
                Some(Err(err))
            }
        }
    }
}

pub struct FramesIterZeroed<'f, F, A> {
    inner: FramesIter<'f, F, A>,
}
impl<'f, F, A> Iterator for FramesIterZeroed<'f, F, A>
where
    A: Arch,
    F: FrameAllocator<A>,
{
    type Item = crate::Result<Range<PhysicalAddress>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.next()? {
            Ok(range) => {
                let mut ptr = range.start.as_raw() as *mut u64;
                unsafe {
                    let end = ptr.add(range.size() / size_of::<u64>());
                    while ptr < end {
                        ptr.write_volatile(0);
                        ptr = ptr.offset(1);
                    }
                }

                Some(Ok(range))
            }
            Err(err) => Some(Err(err)),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{EmulateArch, Error};

    #[test]
    fn single_region_single_frame() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x4000)..top];
        let mut alloc: BumpAllocator<EmulateArch> = unsafe { BumpAllocator::new(&regions) };

        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x1000));
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x2000));
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x3000));
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x4000));
        assert!(matches!(alloc.allocate_frame(), Err(Error::OutOfMemory)));
    }

    #[test]
    fn single_region_multi_frame() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x4000)..top];
        let mut alloc: BumpAllocator<EmulateArch> = unsafe { BumpAllocator::new(&regions) };

        assert_eq!(
            alloc.allocate_frames(3).next().unwrap().unwrap().start,
            top.sub(0x3000)
        );
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x4000));
        assert!(matches!(alloc.allocate_frame(), Err(Error::OutOfMemory)));
    }

    #[test]
    fn multi_region_single_frame() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x9000)..top.sub(0x7000), top.sub(0x4000)..top];
        let mut alloc: BumpAllocator<EmulateArch> = unsafe { BumpAllocator::new(&regions) };

        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x1000));
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x2000));

        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x3000));
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x4000));
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x8000));
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x9000));
        assert!(matches!(alloc.allocate_frame(), Err(Error::OutOfMemory)));
    }

    #[test]
    fn multi_region_multi_frame_contiguous() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x9000)..top.sub(0x7000), top.sub(0x4000)..top];
        let mut alloc: BumpAllocator<EmulateArch> = unsafe { BumpAllocator::new(&regions) };

        assert_eq!(
            alloc.allocate_frames(2).next().unwrap().unwrap().start,
            top.sub(0x2000)
        );
        assert_eq!(
            alloc.allocate_frames(2).next().unwrap().unwrap().start,
            top.sub(0x4000)
        );

        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x8000));
        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x9000));
        assert!(matches!(alloc.allocate_frame(), Err(Error::OutOfMemory)));
    }

    #[test]
    fn multi_region_multi_frame_non_contiguous() {
        let top = PhysicalAddress(usize::MAX);
        let regions = [top.sub(0x9000)..top.sub(0x7000), top.sub(0x4000)..top];
        let mut alloc: BumpAllocator<EmulateArch> = unsafe { BumpAllocator::new(&regions) };

        assert_eq!(
            alloc.allocate_frames(3).next().unwrap().unwrap().start,
            top.sub(0x3000)
        );

        {
            let mut non_contiguous = alloc.allocate_frames(2);
            assert_eq!(
                non_contiguous.next().unwrap().unwrap().start,
                top.sub(0x4000)
            );
            assert_eq!(
                non_contiguous.next().unwrap().unwrap().start,
                top.sub(0x8000)
            );
            assert!(non_contiguous.next().is_none());
        }

        assert_eq!(alloc.allocate_frame().unwrap(), top.sub(0x9000));
        assert!(matches!(alloc.allocate_frame(), Err(Error::OutOfMemory)));
    }
}
