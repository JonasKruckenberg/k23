use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::{cmp, fmt};

use arrayvec::ArrayVec;
use lock_api::Mutex;

use crate::arch::Arch;
use crate::frame_allocator::{AllocError, FrameAllocator};
use crate::{AddressRangeExt, PhysMap, PhysicalAddress};

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
    frame_size: NonZeroUsize,
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
            frame_size: NonZeroUsize::new(A::GRANULE_SIZE).unwrap(),
        }
    }

    /// Returns the array of "regular" physical memory regions managed by this allocator.
    pub fn regions(&self) -> ArrayVec<Range<PhysicalAddress>, MAX_REGIONS> {
        self.inner.lock().regions.clone()
    }

    /// Returns an iterator over the "free" (not allocated) portions of  physical memory regions
    /// managed by this allocator.
    pub fn free_regions(&self) -> impl Iterator<Item=Range<PhysicalAddress>> {
        let inner = self.inner.lock();

        FreeRegions {
            offset: inner.offset,
            inner: inner.regions.clone().into_iter(),
        }
    }

    /// Returns an iterator over the "used" (allocated) portions of  physical memory regions
    /// managed by this allocator.
    pub fn used_regions(&self) -> impl Iterator<Item=Range<PhysicalAddress>> {
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
    fn allocate(
        &self,
        layout: Layout,
    ) -> Result<impl Iterator<Item=Range<PhysicalAddress>>, AllocError> {
        assert!(
            layout.align() >= self.frame_size.get(),
            "BootstrapAllocator only supports page-aligned allocations"
        );

        Ok(self.inner.lock().allocate(layout)?.into_iter())
    }

    fn allocate_zeroed(
        &self,
        layout: Layout,
        physmap: &PhysMap,
        arch: &impl Arch,
    ) -> Result<impl Iterator<Item=Range<PhysicalAddress>>, AllocError> {
        assert!(
            layout.align() >= self.frame_size.get(),
            "BootstrapAllocator only supports page-aligned allocations"
        );

        let chunks = self.inner.lock().allocate(layout)?;

        // iterate over all chunks and fill them with zeroes
        chunks.iter().for_each(|chunk_phys| {
            let chunk_virt = physmap.phys_to_virt_range(chunk_phys.clone());

            // Safety: we just allocated the chunk above
            unsafe {
                arch.write_bytes(chunk_virt.start, 0, chunk_virt.len());
            }
        });

        Ok(chunks.into_iter())
    }

    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        assert!(
            layout.align() >= self.frame_size.get(),
            "BootstrapAllocator only supports page-aligned allocations"
        );

        self.inner.lock().allocate_contiguous(layout)
    }

    unsafe fn deallocate(&self, _block: PhysicalAddress, _layout: Layout) {
        unimplemented!("BootstrapAllocator does not support deallocation")
    }

    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>) {
        (self.frame_size, None)
    }
}

impl<const MAX_REGIONS: usize> BootstrapAllocatorInner<MAX_REGIONS> {
    fn allocate(
        &mut self,
        layout: Layout,
    ) -> Result<ArrayVec<Range<PhysicalAddress>, MAX_REGIONS>, AllocError> {
        let mut requested_size_bytes = layout.pad_to_align().size();
        let mut offset = self.offset;
        let mut chunks = ArrayVec::new();

        for region in self.regions.iter().rev() {
            let region_size_bytes = region.len();

            // only consider regions that we haven't already exhausted
            if offset < region_size_bytes {
                let alloc_size_bytes = cmp::min(requested_size_bytes, region_size_bytes - offset);

                let base = region.end.sub(offset + alloc_size_bytes);
                self.offset += alloc_size_bytes;
                requested_size_bytes -= alloc_size_bytes;

                chunks.push(Range::from_start_len(base, alloc_size_bytes));

                if requested_size_bytes == 0 {
                    return Ok(chunks);
                }
            } else {
                offset -= region_size_bytes;
            }
        }

        Err(AllocError)
    }

    fn allocate_contiguous(&mut self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let requested_size_bytes = layout.pad_to_align().size();
        let mut offset = self.offset;

        for region in self.regions.iter().rev() {
            let region_size_bytes = region.len();

            // only consider regions that we haven't already exhausted
            if offset < region_size_bytes {
                // Allocating a contiguous range has different requirements than "regular" allocation
                // contiguous are rare and often happen in very critical paths where e.g. virtual
                // memory is not available yet. So we rather waste some memory than outright crash.
                if region_size_bytes - offset < requested_size_bytes {
                    log::warn!(
                        "Skipped memory region {region:?} since it was too small to fulfill request for {requested_size_bytes} bytes. Wasted {} bytes in the process...",
                        region_size_bytes - offset
                    );

                    self.offset += region_size_bytes - offset;
                    offset = 0;
                    continue;
                }

                let base = region.end.sub(offset + requested_size_bytes);
                self.offset += requested_size_bytes;

                debug_assert!(
                    base.is_aligned_to(layout.align()),
                    "allocated address {base} is not aligned to {layout:?}. {self:?}"
                );
                return Ok(base);
            } else {
                offset -= region_size_bytes;
            }
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
    use crate::{archtest, PhysMap, PhysicalAddress};

    fn assert_zeroed(frame: PhysicalAddress, bytes: usize, physmap: &PhysMap, arch: &impl Arch) {
        let frame = unsafe { arch.read_bytes(physmap.phys_to_virt(frame), bytes) };

        assert!(frame.iter().all(|byte| *byte == 0));
    }

    archtest! {
        // Assert that the BootstrapAllocator can allocate frames
        #[test_log::test]
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
        #[test_log::test]
        fn allocate_contiguous_zeroed_bare<A: Arch>() {
            let (machine, _) = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                .with_memory_regions([0x3000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions());

            let arch = EmulateArch::new(machine);

            let physmap = PhysMap::new_bootstrap();

            // Based on the memory of the machine we set up above, we expect the allocator to
            // yield 3 pages.

            let frame = frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();
            assert_zeroed(frame, A::GRANULE_SIZE, &physmap, &arch);

            let frame = frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();
            assert_zeroed(frame, A::GRANULE_SIZE, &physmap, &arch);

            let frame = frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();
            assert_zeroed(frame, A::GRANULE_SIZE, &physmap, &arch);

            // assert that we're out of memory
            frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap_err();
        }

        // Assert that the BootstrapAllocator can allocate frames
        #[test_log::test]
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

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use crate::address_range::AddressRangeExt;
    use crate::arch::Arch;
    use crate::frame_allocator::FrameAllocator;
    use crate::for_every_arch;
    use core::alloc::Layout;
    use crate::{KIB,GIB};
    use crate::bootstrap::{BootstrapAllocator, DEFAULT_MAX_REGIONS};
    use crate::test_utils::proptest::region_sizes;
    use crate::test_utils::MachineBuilder;

    for_every_arch!(A => {
        proptest! {
            #[test_log::test]
            fn allocate_exhaust(region_sizes in region_sizes(1..DEFAULT_MAX_REGIONS, 4*KIB, 16*GIB)) {
                let (machine, _) = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                    .with_memory_regions(region_sizes.clone())
                    .finish();

                let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                    BootstrapAllocator::new::<A>(machine.memory_regions());

                let total_size = region_sizes.iter().sum();

                let res = frame_allocator
                    .allocate(Layout::from_size_align(total_size, A::GRANULE_SIZE).unwrap());
                prop_assert!(res.is_ok());
                let chunks = res.unwrap();

                let chunks: Vec<_> = chunks.collect();

                // assert the total size is what we expect
                let allocated_size: usize = chunks.iter().map(|chunk| chunk.len()).sum();
                prop_assert!(allocated_size >= total_size);

                // assert each chunk is aligned correctly
                for chunk in chunks.iter() {
                    prop_assert!(chunk.start.is_aligned_to(A::GRANULE_SIZE));
                }
            }

            #[test_log::test]
            fn allocate_contiguous_exhaust(region_sizes in region_sizes(1..DEFAULT_MAX_REGIONS, 1*GIB, 16*GIB)) {
                let (machine, _) = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                    .with_memory_regions(region_sizes.clone())
                    .finish();

                let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                    BootstrapAllocator::new::<A>(machine.memory_regions());

                let total_size = region_sizes.iter().sum();

                for _ in (0..total_size).step_by(1*GIB) {
                    let res = frame_allocator.allocate_contiguous(Layout::from_size_align(1*GIB, 1*GIB).unwrap());
                    prop_assert!(res.is_ok());
                    let base = res.unwrap();
                    prop_assert!(base.is_aligned_to(1*GIB));
                }
            }
        }
    });
}
