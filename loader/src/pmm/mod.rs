pub mod arch;
mod error;

use bitflags::bitflags;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::{cmp, fmt, iter, slice};
pub use error::Error;

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);
impl VirtualAddress {
    #[must_use]
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    #[must_use]
    #[allow(clippy::cast_sign_loss)]
    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    #[must_use]
    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        assert!(!overflow, "virtual address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        assert!(!overflow, "virtual address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        assert!(!overflow, "virtual address underflow");
        out
    }

    #[must_use]
    pub const fn as_raw(&self) -> usize {
        self.0
    }

    #[must_use]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(
            align.is_power_of_two(),
            "is_aligned_to: align is not a power-of-two"
        );

        self.as_raw() & (align - 1) == 0
    }

    #[must_use]
    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

    #[must_use]
    pub const fn align_up(self, alignment: usize) -> Self {
        Self((self.0 + alignment - 1) & !(alignment - 1))
    }
}
impl fmt::Display for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}
impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);
impl PhysicalAddress {
    #[must_use]
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    #[must_use]
    #[allow(clippy::cast_sign_loss)]
    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    #[must_use]
    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        assert!(!overflow, "physical address overflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        assert!(!overflow, "physical address underflow");
        Self(out)
    }

    #[must_use]
    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        assert!(!overflow, "physical address underflow");
        out
    }

    #[must_use]
    pub const fn as_raw(&self) -> usize {
        self.0
    }

    #[must_use]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(
            align.is_power_of_two(),
            "is_aligned_to: align is not a power-of-two"
        );

        self.as_raw() & (align - 1) == 0
    }

    #[must_use]
    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

    #[must_use]
    pub const fn align_up(self, alignment: usize) -> Self {
        Self((self.0 + alignment - 1) & !(alignment - 1))
    }
}
impl fmt::Display for PhysicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}
impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct Flags: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

pub trait FrameAllocator {
    fn allocate_frames_contiguous(
        &mut self,
        frames: usize,
    ) -> Result<(PhysicalAddress, NonZeroUsize), Error>;
    fn allocate_frame(&mut self) -> Result<PhysicalAddress, Error> {
        let (phys, frames) = self.allocate_frames_contiguous(1)?;
        debug_assert_eq!(frames.get(), 1);
        Ok(phys)
    }
    fn allocate_frame_zeroed(&mut self) -> Result<PhysicalAddress, Error> {
        let (phys, frames) = self.allocate_frames_contiguous(1)?;
        debug_assert_eq!(frames.get(), 1);
        zero_frames(phys.as_raw() as _, frames);
        Ok(phys)
    }
    fn allocate_frames(&mut self, frames: usize) -> FramesIter<'_> where Self: Sized {
        FramesIter {
            alloc: self,
            remaining: frames,
            zeroed: false,
        }
    }
    fn allocate_frames_zeroed(&mut self, frames: usize) -> FramesIter<'_> where Self: Sized {
        FramesIter {
            alloc: self,
            remaining: frames,
            zeroed: true,
        }
    }
}

pub struct FramesIter<'a> {
    alloc: &'a mut dyn FrameAllocator,
    remaining: usize,
    zeroed: bool,
}

impl<'a> FramesIter<'a> {
    pub fn alloc_mut(&mut self) -> &mut dyn FrameAllocator {
        self.alloc
    }
}
impl<'a> Iterator for FramesIter<'a> {
    type Item = Result<(PhysicalAddress, NonZeroUsize), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        match self.alloc.allocate_frames_contiguous(self.remaining) {
            Ok((phys, frames)) => {
                self.remaining -= frames.get();

                if self.zeroed {
                    zero_frames(phys.as_raw() as _, frames)
                }

                Some(Ok((phys, frames)))
            }
            Err(err) => {
                self.remaining = 0;
                Some(Err(err))
            }
        }
    }
}

pub fn zero_frames(mut ptr: *mut u64, num_frames: NonZeroUsize) {
    unsafe {
        let end = ptr.add((num_frames.get() * arch::PAGE_SIZE) / size_of::<u64>());
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }
}

pub struct BumpAllocator<'a> {
    regions: &'a [Range<PhysicalAddress>],
    // offset from the top of memory regions
    offset: usize,
    lower_bound: PhysicalAddress,
}
impl<'a> BumpAllocator<'a> {
    /// Create a new frame allocator over a given set of physical memory regions.
    #[must_use]
    pub fn new(regions: &'a [Range<PhysicalAddress>]) -> Self {
        Self {
            regions,
            offset: 0,
            lower_bound: PhysicalAddress(0),
        }
    }

    /// Create a new frame allocator over a given set of physical memory regions.
    #[must_use]
    pub fn new_with_lower_bound(
        regions: &'a [Range<PhysicalAddress>],
        lower_bound: PhysicalAddress,
    ) -> Self {
        Self {
            regions,
            offset: 0,
            lower_bound,
        }
    }

    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    #[must_use]
    pub fn regions(&self) -> &'a [Range<PhysicalAddress>] {
        self.regions
    }

    #[must_use]
    pub fn free_regions(&self) -> FreeRegions<'_> {
        FreeRegions {
            offset: self.offset,
            inner: self.regions().iter().rev().cloned(),
        }
    }

    #[must_use]
    pub fn used_regions(&self) -> UsedRegions<'_> {
        UsedRegions {
            offset: self.offset,
            inner: self.regions().iter().rev().cloned(),
        }
    }
}

impl<'a> FrameAllocator for BumpAllocator<'a> {
    fn allocate_frames_contiguous(
        &mut self,
        frames: usize,
    ) -> Result<(PhysicalAddress, NonZeroUsize), Error> {
        let requested_size = frames * arch::PAGE_SIZE;
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.end.as_raw() - region.start.as_raw();

            // only consider regions that we haven't already exhausted
            if offset < region_size {
                let alloc_size = cmp::min(requested_size, region_size - offset);

                let frame = region.end.sub(offset + alloc_size);

                if frame <= self.lower_bound {
                    log::error!(
                        "Allocation would have crossed `lower_bound`: {} <= {}",
                        frame,
                        self.lower_bound
                    );
                    return Err(Error::OutOfMemory);
                }

                self.offset += alloc_size;

                return Ok((frame, NonZeroUsize::new(alloc_size / arch::PAGE_SIZE).unwrap()));
            }

            offset -= region_size;
        }

        Err(Error::OutOfMemory)
    }
}


pub struct FreeRegions<'a> {
    offset: usize,
    inner: iter::Cloned<iter::Rev<slice::Iter<'a, Range<PhysicalAddress>>>>,
}

impl Iterator for FreeRegions<'_> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut region = self.inner.next()?;
            let region_size = region.end.as_raw() - region.start.as_raw();
            // keep advancing past already fully used memory regions
            if self.offset >= region_size {
                self.offset -= region_size;
                continue;
            } else if self.offset > 0 {
                region.end = region.end.sub(self.offset);
                self.offset = 0;
            }

            return Some(region);
        }
    }
}

pub struct UsedRegions<'a> {
    offset: usize,
    inner: iter::Cloned<iter::Rev<slice::Iter<'a, Range<PhysicalAddress>>>>,
}

impl Iterator for UsedRegions<'_> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut region = self.inner.next()?;
        let region_size = region.end.as_raw() - region.start.as_raw();

        if self.offset >= region_size {
            Some(region)
        } else if self.offset > 0 {
            region.start = region.end.sub(self.offset);
            self.offset = 0;

            Some(region)
        } else {
            None
        }
    }
}