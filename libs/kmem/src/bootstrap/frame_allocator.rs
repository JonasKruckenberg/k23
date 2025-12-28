use core::alloc::Layout;
use core::fmt;
use core::num::NonZeroUsize;
use core::ops::Range;

use arrayvec::ArrayVec;
use lock_api::Mutex;

use crate::arch::Arch;
use crate::frame_allocator::{AllocError, FrameAllocator};
use crate::{AddressRangeExt, PhysicalAddress};

pub const DEFAULT_MAX_REGIONS: usize = 16;

/// Simple bump allocator (cannot free) that can be used to allocate physical memory frames early during system
/// bootstrap.
///
/// This allocator supports discontiguous physical memory by default. By default, up to [`DEFAULT_MAX_REGIONS`]
/// but this limit can be adjusted by explicitly specifying the const-generic parameter.
pub struct BootstrapAllocator<R, const MAX_REGIONS: usize = DEFAULT_MAX_REGIONS>
where
    R: lock_api::RawMutex,
{
    inner: Mutex<R, BootstrapAllocatorInner<MAX_REGIONS>>,
    // we make a "snapshot" of the translation granule size during construction so that the allocator
    // itself doesn't need to be generic over `Arch`.
    frame_size: usize,
}

#[derive(Debug)]
struct BootstrapAllocatorInner<const MAX_REGIONS: usize> {
    /// The discontiguous regions of "regular" physical memory that we can use for allocation.
    regions: ArrayVec<Range<PhysicalAddress>, MAX_REGIONS>,
    /// offset from the top of memory regions
    offset: usize,
}

impl<R, const MAX_REGIONS: usize> fmt::Debug for BootstrapAllocator<R, MAX_REGIONS>
where
    R: lock_api::RawMutex,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BootstrapAllocator")
            .field("regions", &self.inner.lock())
            .field("frame_size", &self.frame_size)
            .finish()
    }
}

impl<R, const MAX_REGIONS: usize> BootstrapAllocator<R, MAX_REGIONS>
where
    R: lock_api::RawMutex,
{
    /// Constructs a new bootstrap frame allocator from the given regions of physical memory.
    ///
    /// # Panics
    ///
    /// Panics if given regions overlap.
    pub fn new<A: Arch>(mut regions: ArrayVec<Range<PhysicalAddress>, MAX_REGIONS>) -> Self {
        if regions.len() > 1 {
            regions.sort_unstable_by_key(|region| region.start);

            let mut iter = regions.as_mut_slice().windows(2);

            while let Some([cur, next]) = iter.next() {
                assert!(
                    !cur.overlaps(next),
                    "regions {cur:#x?} and {next:#x?} overlap"
                );
            }
        }

        regions
            .iter_mut()
            .for_each(|region| *region = region.clone().align_in(A::GRANULE_SIZE));

        Self {
            inner: Mutex::new(BootstrapAllocatorInner { regions, offset: 0 }),
            frame_size: A::GRANULE_SIZE,
        }
    }

    /// Returns the array of "regular" physical memory regions managed by this allocator.
    pub fn regions(&self) -> ArrayVec<Range<PhysicalAddress>, MAX_REGIONS> {
        self.inner.lock().regions.clone()
    }

    /// Returns an iterator over the "free" (not allocated) portions of  physical memory regions
    /// managed by this allocator.
    pub fn free_regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        let inner = self.inner.lock();

        FreeRegions {
            offset: inner.offset,
            inner: inner.regions.clone().into_iter(),
        }
    }

    /// Returns an iterator over the "used" (allocated) portions of  physical memory regions
    /// managed by this allocator.
    pub fn used_regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        let inner = self.inner.lock();

        UsedRegions {
            offset: inner.offset,
            inner: inner.regions.clone().into_iter(),
        }
    }

    /// Returns the number of allocated bytes.
    pub fn usage(&self) -> usize {
        self.inner.lock().offset
    }
}

// Safety: bootstrap allocator manages raw physical memory regions, they remain valid theoretically
// forever we merely hand out "land claims" to it.
unsafe impl<R, const MAX_REGIONS: usize> FrameAllocator for BootstrapAllocator<R, MAX_REGIONS>
where
    R: lock_api::RawMutex,
{
    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        assert_eq!(
            layout.align(),
            self.frame_size,
            "BootstrapAllocator only supports page-aligned allocations"
        );

        self.inner.lock().allocate(layout)
    }

    unsafe fn deallocate(&self, _block: PhysicalAddress, _layout: Layout) {
        unimplemented!("BootstrapAllocator does not support deallocation")
    }

    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>) {
        (NonZeroUsize::new(self.frame_size).unwrap(), None)
    }
}

impl<const MAX_REGIONS: usize> BootstrapAllocatorInner<MAX_REGIONS> {
    fn allocate(&mut self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let requested_size = layout.pad_to_align().size();
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size = region.len();

            // only consider regions that we haven't already exhausted
            if offset < region_size {
                // Allocating a contiguous range has different requirements than "regular" allocation
                // contiguous are rare and often happen in very critical paths where e.g. virtual
                // memory is not available yet. So we rather waste some memory than outright crash.
                if region_size - offset < requested_size {
                    log::warn!(
                        "Skipped memory region {region:?} since it was too small to fulfill request for {requested_size} bytes. Wasted {} bytes in the process...",
                        region_size - offset
                    );

                    self.offset += region_size - offset;
                    offset = 0;
                    continue;
                }

                let frame = region.end.sub(offset + requested_size);
                self.offset += requested_size;
                return Ok(frame);
            }

            offset -= region_size;
        }

        Err(AllocError)
    }
}

pub struct FreeRegions<const MAX_REGIONS: usize> {
    offset: usize,
    inner: arrayvec::IntoIter<Range<PhysicalAddress>, { MAX_REGIONS }>,
}

impl<const MAX_REGIONS: usize> Iterator for FreeRegions<MAX_REGIONS> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let mut region = self.inner.next()?;
            // keep advancing past already fully used memory regions
            let region_size = region.len();

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

pub struct UsedRegions<const MAX_REGIONS: usize> {
    offset: usize,
    inner: arrayvec::IntoIter<Range<PhysicalAddress>, { MAX_REGIONS }>,
}

impl<const MAX_REGIONS: usize> Iterator for UsedRegions<MAX_REGIONS> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut region = self.inner.next()?;

        if self.offset >= region.len() {
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

#[cfg(test)]
mod tests {
    use crate::arch::Arch;
    use crate::bootstrap::BootstrapAllocator;
    use crate::frame_allocator::FrameAllocator;
    use crate::test_utils::{EmulateArch, MachineBuilder};
    use crate::{PhysMap, archtest};

    archtest! {
        // Assert that the BootstrapAllocator can allocate frames
        #[test]
        fn allocate_contiguous<A: Arch>() {
            let (machine, _) = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                .with_memory_regions([0x3000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions());

            // Based on the memory of the machine we set up above, we expect the allocator to
            // yield 3 pages.

            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            // assert that we're out of memory
            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap_err();
        }

        // Assert that the BootstrapAllocator can allocate zeroed frames in
        // bootstrap (bare, before paging is enabled) mode.
        #[test]
        fn allocate_contiguous_zeroed_bare<A: Arch>() {
            let (machine, _) = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                .with_memory_regions([0x3000])
                .finish();

            println!("{machine:?}");

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions());

            let arch = EmulateArch::new(machine);

            let physmap = PhysMap::new_bootstrap();

            // Based on the memory of the machine we set up above, we expect the allocator to
            // yield 3 pages.

            frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();

            frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();

            frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();

            // assert that we're out of memory
            frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap_err();
        }

        // Assert that the BootstrapAllocator can allocate frames
        #[test]
        fn allocate_contiguous_multiple_regions<A: Arch>() {
            let (machine, _) = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                .with_memory_regions([0x3000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions());

            // Based on the memory of the machine we set up above, we expect the allocator to
            // yield 3 pages.

            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            // assert that we're out of memory
            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap_err();
        }
    }
}
