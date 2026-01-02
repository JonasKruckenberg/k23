use core::alloc::Layout;
use core::cmp::Ordering;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::{cmp, fmt, iter};

use k23_arrayvec::ArrayVec;
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
    min_align: NonZeroUsize,
}

#[derive(Debug)]
struct BootstrapAllocatorInner<const MAX_REGIONS: usize> {
    arenas: ArrayVec<Arena, MAX_REGIONS>,
    current_arena_hint: usize,
}

impl<R, const MAX_REGIONS: usize> fmt::Debug for BootstrapAllocator<R, MAX_REGIONS>
where
    R: lock_api::RawMutex,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BootstrapAllocator")
            .field("inner", &self.inner.lock())
            .field("min_align", &self.min_align)
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

        let mut largest_region_idx = 0;
        let mut largest_region_size = 0;
        let arenas: ArrayVec<_, MAX_REGIONS> = regions
            .into_iter()
            .enumerate()
            .map(|(i, region)| {
                let region = region.align_in(A::GRANULE_SIZE);

                if region.len() > largest_region_size {
                    largest_region_size = region.len();
                    largest_region_idx = i;
                }

                Arena {
                    // we allocate from the top of each region downward
                    ptr: region.end,
                    region,
                }
            })
            .collect();

        Self {
            inner: Mutex::new(BootstrapAllocatorInner {
                arenas,
                current_arena_hint: largest_region_idx,
            }),
            min_align: NonZeroUsize::new(A::GRANULE_SIZE).unwrap(),
        }
    }

    /// Returns the array of "regular" physical memory regions managed by this allocator.
    #[inline]
    pub fn regions(&self) -> ArrayVec<Range<PhysicalAddress>, MAX_REGIONS> {
        self.inner
            .lock()
            .arenas
            .iter()
            .map(|arena| arena.region.clone())
            .collect()
    }

    /// Returns the remaining capacity (free bytes) of this allocator in bytes
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacities().into_iter().sum()
    }

    /// Returns the remaining capacity of each physical memory region.
    #[inline]
    pub fn capacities(&self) -> ArrayVec<usize, MAX_REGIONS> {
        self.inner
            .lock()
            .arenas
            .iter()
            .map(|region| region.capacity())
            .collect()
    }

    /// Returns the number of allocated bytes.
    #[inline]
    pub fn usage(&self) -> usize {
        self.usages().into_iter().sum()
    }

    /// Returns the number of allocated bytes of each physical memory region.
    #[inline]
    pub fn usages(&self) -> ArrayVec<usize, MAX_REGIONS> {
        self.inner
            .lock()
            .arenas
            .iter()
            .map(|region| region.usage())
            .collect()
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
    ) -> Result<impl ExactSizeIterator<Item = Range<PhysicalAddress>>, AllocError> {
        let mut inner = self.inner.lock();

        if let Some(p) = inner.allocate_contiguous_fast(self.min_align, layout) {
            let block = Range::from_start_len(p, layout.size());

            Ok(Blocks::One(iter::once(block)))
        } else {
            let blocks = inner
                .allocate_slow(self.min_align, layout)
                .ok_or(AllocError)?;

            Ok(Blocks::Multiple(blocks.into_iter()))
        }
    }

    #[inline(always)]
    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let mut inner = self.inner.lock();

        if let Some(p) = inner.allocate_contiguous_fast(self.min_align, layout) {
            Ok(p)
        } else {
            inner
                .allocate_contiguous_slow(self.min_align, layout)
                .ok_or(AllocError)
        }
    }

    unsafe fn deallocate(&self, _block: PhysicalAddress, _layout: Layout) {
        unimplemented!("BootstrapAllocator does not support deallocation");
    }
}

impl<const MAX_REGIONS: usize> BootstrapAllocatorInner<MAX_REGIONS> {
    /// Fast-path for allocation from the "current" arena. Most modern machines have a single large
    /// physical memory region. During creation, we determine the largest physical memory region
    /// and designate it as the "current" arena.
    ///
    /// This means this fast-path can fulfill the vast majority of requests with a single allocation
    /// from this "main" arena.
    #[inline(always)]
    fn allocate_contiguous_fast(
        &mut self,
        min_align: NonZeroUsize,
        layout: Layout,
    ) -> Option<PhysicalAddress> {
        self.arenas[self.current_arena_hint].allocate(min_align, layout)
    }

    /// Cold-path when we have exhausted the capacity of the current region and need to consider
    /// other regions.
    #[inline(never)]
    #[cold]
    fn allocate_contiguous_slow(
        &mut self,
        min_align: NonZeroUsize,
        layout: Layout,
    ) -> Option<PhysicalAddress> {
        let current_arena_hint = self.current_arena_hint;

        // NB: we know this method is called when the "current region" (as indicated by the current region hint)
        // is exhausted. We therefore begin our search at the next region (offset 1..) wrapping around
        // at the end to double-check previous regions (they might have capacity still). But we still
        // don't double-check the "current region" as we know it cant fit `layout`.
        for offset in 1..self.arenas.len() {
            let i = (current_arena_hint + offset) % self.arenas.len();

            // only attempt to allocate if the region has any capacity
            if self.arenas[i].has_capacity()
                && let Some(block) = self.arenas[i].allocate(min_align, layout)
            {
                self.current_arena_hint = i;

                return Some(block);
            }
        }

        None
    }

    /// Cold-path for discontiguous allocations when we have exhausted the capacity of the current
    /// region and need to consider other regions.
    #[inline(never)]
    #[cold]
    fn allocate_slow(
        &mut self,
        min_align: NonZeroUsize,
        layout: Layout,
    ) -> Option<ArrayVec<Range<PhysicalAddress>, MAX_REGIONS>> {
        let mut blocks: ArrayVec<_, MAX_REGIONS> = ArrayVec::new();
        let mut remaining_bytes = layout.size();
        let current_arena_hint = self.current_arena_hint;

        for offset in 0..self.arenas.len() {
            let i = (current_arena_hint + offset) % self.arenas.len();

            // only attempt to allocate if the region has any capacity
            if self.arenas[i].has_capacity() {
                // attempt to allocate as big of a block as we can
                let requested_size = cmp::min(remaining_bytes, self.arenas[i].capacity());
                let layout = Layout::from_size_align(requested_size, layout.align()).unwrap();

                if let Some(block) = self.arenas[i].allocate(min_align, layout) {
                    self.current_arena_hint = i;
                    remaining_bytes -= requested_size;

                    blocks.push(Range::from_start_len(block, requested_size));
                }
            }

            // if - through this loop iteration - we fully allocated the required memory
            // return the blocks
            if remaining_bytes == 0 {
                return Some(blocks);
            }
        }

        log::trace!(
            "failed to allocate layout {layout:?} capacities={:#?} usages={:#?}",
            self.arenas
                .iter()
                .map(|arena| arena.capacity())
                .collect::<ArrayVec<_, MAX_REGIONS>>(),
            self.arenas
                .iter()
                .map(|arena| arena.usage())
                .collect::<ArrayVec<_, MAX_REGIONS>>()
        );

        // if we've gone through all regions without fully allocating the required memory we cant
        // satisfy this allocation request.
        // we might have allocated some blocks though, so lets go and clean them up now

        for block in blocks {
            let region = self
                .arenas
                .iter_mut()
                .find(|region| region.region.overlaps(&block))
                .unwrap_or_else(|| {
                    panic!("block {block:?} must belong to an arena. this is a bug!")
                });

            region.deallocate_if_last(block);
        }

        None
    }
}

/// Manages a contiguous region of physical memory.
struct Arena {
    region: Range<PhysicalAddress>,
    ptr: PhysicalAddress,
}

impl fmt::Debug for Arena {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arena")
            .field("region", &self.region)
            .field("ptr", &self.ptr)
            .field("<capacity>", &(self.free(), self.capacity()))
            .field("<usage>", &(self.used(), self.usage()))
            .finish()
    }
}

impl Arena {
    /// Returns the number of bytes left to allocate
    #[inline]
    pub fn capacity(&self) -> usize {
        self.ptr.offset_from_unsigned(self.region.start)
    }

    /// Returns true if this arena has any capacity left
    #[inline]
    fn has_capacity(&self) -> bool {
        self.capacity() > 0
    }

    /// Returns the number of bytes allocated from this arena
    #[inline]
    pub fn usage(&self) -> usize {
        self.region.end.offset_from_unsigned(self.ptr)
    }

    /// Returns the used (allocated) slice of the physical memory region managed by this arena
    #[inline]
    pub fn used(&self) -> Range<PhysicalAddress> {
        self.ptr..self.region.end
    }

    /// Returns the free (not allocated) slice of the physical memory region managed by this arena
    #[inline]
    pub fn free(&self) -> Range<PhysicalAddress> {
        self.region.start..self.ptr
    }

    /// Deallocates a given memory block IF it is the last block that was allocated from this arena.
    ///
    /// # Panics
    ///
    /// Panics if the block was not the last allocated block.
    fn deallocate_if_last(&mut self, block: Range<PhysicalAddress>) {
        if self.ptr == block.start {
            self.ptr = block.end;
        } else {
            panic!("can only free last allocated block");
        }
    }

    /// Attempt to allocate enough memory to satisfy the size and alignment requirements of `layout`.
    #[inline]
    fn allocate(&mut self, min_align: NonZeroUsize, layout: Layout) -> Option<PhysicalAddress> {
        debug_assert!(
            self.region.start <= self.ptr && self.ptr <= self.region.end,
            "bump pointer {:?} should in region range {}..={}",
            self.ptr,
            self.region.start,
            self.region.end
        );
        debug_assert!(
            self.ptr.is_aligned_to(min_align.get()),
            "bump pointer {:?} should be aligned to the minimum alignment of {min_align:#x}",
            self.ptr
        );

        let aligned_ptr = match layout.align().cmp(&min_align.get()) {
            Ordering::Less => {
                // the requested alignment is smaller than our minimum alignment
                // we need to round up the requested size to our minimum alignment
                let aligned_size = round_up_to(layout.size(), min_align.get())?;

                if self.capacity() < aligned_size {
                    return None;
                }

                self.ptr.wrapping_sub(aligned_size)
            }
            Ordering::Equal => {
                // the requested alignment is equal to our minimum alignment

                // round up the layout size to be a multiple of the layout's alignment
                // Safety: `Layout` guarantees that rounding the size up to its align cannot overflow
                let aligned_size = unsafe { round_up_to_unchecked(layout.size(), layout.align()) };

                if self.capacity() < aligned_size {
                    return None;
                }

                self.ptr.wrapping_sub(aligned_size)
            }
            Ordering::Greater => {
                // the requested alignment is greater than our minimum alignment

                // round up the layout size to be a multiple of the layout's alignment.
                // Safety: `Layout` guarantees that rounding the size up to its align cannot overflow
                let aligned_size = unsafe { round_up_to_unchecked(layout.size(), layout.align()) };

                let aligned_ptr = self.ptr.align_down(layout.align());

                // NB: we're not using .capacity() here because we actually care about the capacity
                // that's left *after* aligning the bump pointer down
                let capacity = aligned_ptr.offset_from_unsigned(self.region.start);

                if aligned_ptr < self.region.start || capacity < aligned_size {
                    return None;
                }

                aligned_ptr.wrapping_sub(aligned_size)
            }
        };

        debug_assert!(
            aligned_ptr.is_aligned_to(layout.align()),
            "pointer {aligned_ptr:?} should be aligned to layout alignment of {}",
            layout.align()
        );
        debug_assert!(
            aligned_ptr.is_aligned_to(min_align.get()),
            "pointer {aligned_ptr:?} should be aligned to minimum alignment of {min_align}",
        );
        debug_assert!(
            self.region.start <= aligned_ptr && aligned_ptr <= self.ptr,
            "pointer {aligned_ptr:?} should be in range {:?}..={:?}",
            self.region.start,
            self.ptr
        );

        self.ptr = aligned_ptr;

        Some(aligned_ptr)
    }
}

#[inline]
const fn round_up_to(n: usize, divisor: usize) -> Option<usize> {
    debug_assert!(divisor > 0);
    debug_assert!(divisor.is_power_of_two());

    match n.checked_add(divisor - 1) {
        Some(x) => Some(x & !(divisor - 1)),
        None => None,
    }
}

/// Like `round_up_to` but turns overflow into undefined behavior rather than
/// returning `None`.
///
/// # Safety:
///
/// This results in undefined behavior when `n  + (divisor - 1) > usize::MAX` or n  + (divisor - 1) < usize::MIN` i.e. when round_up_to would return None.
#[inline]
unsafe fn round_up_to_unchecked(n: usize, divisor: usize) -> usize {
    match round_up_to(n, divisor) {
        Some(x) => x,
        None => {
            debug_assert!(false, "round_up_to_unchecked failed");

            // Safety: ensured by caller
            unsafe { core::hint::unreachable_unchecked() }
        }
    }
}

enum Blocks<const MAX: usize> {
    One(iter::Once<Range<PhysicalAddress>>),
    Multiple(k23_arrayvec::IntoIter<Range<PhysicalAddress>, MAX>),
}

impl<const MAX: usize> Iterator for Blocks<MAX> {
    type Item = Range<PhysicalAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Blocks::One(iter) => iter.next(),
            Blocks::Multiple(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Blocks::One(iter) => iter.size_hint(),
            Blocks::Multiple(iter) => iter.size_hint(),
        }
    }
}

impl<const MAX: usize> ExactSizeIterator for Blocks<MAX> {}

#[cfg(test)]
mod tests {
    use core::alloc::Layout;

    use crate::address_range::AddressRangeExt;
    use crate::arch::Arch;
    use crate::bootstrap::BootstrapAllocator;
    use crate::frame_allocator::FrameAllocator;
    use crate::test_utils::{EmulateArch, Machine, MachineBuilder};
    use crate::{GIB, PhysMap, PhysicalAddress, archtest};

    fn assert_zeroed(frame: PhysicalAddress, bytes: usize, physmap: &PhysMap, arch: &impl Arch) {
        let frame = unsafe { arch.read_bytes(physmap.phys_to_virt(frame), bytes) };

        assert!(frame.iter().all(|byte| *byte == 0));
    }

    archtest! {
        // Assert that the BootstrapAllocator can allocate frames
        #[test_log::test]
        fn allocate_contiguous_smoke<A: Arch>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([0x2000, 0x1000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions().collect());

            // Based on the memory of the machine we set up above, we expect the allocator to
            // yield 3 pages.

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();
            assert!(frame.is_aligned_to(A::GRANULE_SIZE));

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();
            assert!(frame.is_aligned_to(A::GRANULE_SIZE));

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();
            assert!(frame.is_aligned_to(A::GRANULE_SIZE));

            // assert that we're out of memory
            frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap_err();
        }

        // Assert that the BootstrapAllocator can allocate zeroed frames in
        // bootstrap (bare, before paging is enabled) mode.
        #[test_log::test]
        fn allocate_contiguous_zeroed_smoke<A: Arch>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([0x2000, 0x1000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions().collect());

            let arch = EmulateArch::new(machine);

            let physmap = PhysMap::new_bootstrap();

            // Based on the memory of the machine we set up above, we expect the allocator to
            // yield 3 pages.

            let frame = frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();
            assert!(frame.is_aligned_to(A::GRANULE_SIZE));
            assert_zeroed(frame, A::GRANULE_SIZE, &physmap, &arch);

            let frame = frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();
            assert!(frame.is_aligned_to(A::GRANULE_SIZE));
            assert_zeroed(frame, A::GRANULE_SIZE, &physmap, &arch);

            let frame = frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap();
            assert!(frame.is_aligned_to(A::GRANULE_SIZE));
            assert_zeroed(frame, A::GRANULE_SIZE, &physmap, &arch);

            // assert that we're out of memory
            frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, &physmap, &arch)
                .unwrap_err();
        }

        #[test_log::test]
        fn allocate_smoke<A: Arch>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([0x3000, 0x1000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions().collect());

            let blocks: Vec<_> = frame_allocator
                .allocate(Layout::from_size_align(0x4000, A::GRANULE_SIZE).unwrap())
                .unwrap()
                .collect();

            // assert the total size is what we expect
            let allocated_size: usize = blocks.iter().map(|block| block.len()).sum();
            assert!(allocated_size >= 0x4000);

            // assert each block is aligned correctly
            for block in blocks.iter() {
                assert!(block.start.is_aligned_to(A::GRANULE_SIZE));
            }
        }

        #[test_log::test]
        fn allocate_zeroed_smoke<A: Arch>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([0x3000, 0x1000])
                .finish();

            let arch = EmulateArch::new(machine.clone());

            let physmap = PhysMap::new_bootstrap();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions().collect());

            let blocks: Vec<_> = frame_allocator
                .allocate_zeroed(Layout::from_size_align(0x4000, A::GRANULE_SIZE).unwrap(), &physmap, &arch)
                .unwrap()
                .collect();

            // assert the total size is what we expect
            let allocated_size: usize = blocks.iter().map(|block| block.len()).sum();
            assert!(allocated_size >= 0x4000);

            // assert each block is aligned correctly
            for block in blocks.iter() {
                assert!(block.start.is_aligned_to(A::GRANULE_SIZE));

                assert_zeroed(block.start, block.len(), &physmap, &arch);
            }
        }

        #[test_log::test]
        fn allocate_contiguous_small_alignment<A: Arch>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([0x4000, 0x1000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions().collect());

            let frame = frame_allocator.allocate_contiguous(Layout::from_size_align(A::GRANULE_SIZE, 1).unwrap()).unwrap();

            assert!(frame.is_aligned_to(1));
            assert!(frame.is_aligned_to(A::GRANULE_SIZE));
        }

        #[test_log::test]
        fn allocate_small_alignment<A: Arch>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([0x4000, 0x1000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions().collect());

            let blocks = frame_allocator.allocate(Layout::from_size_align(A::GRANULE_SIZE, 1).unwrap()).unwrap();

            for block in blocks {
                assert!(block.start.is_aligned_to(1));
                assert!(block.start.is_aligned_to(A::GRANULE_SIZE));
            }
        }

        #[test_log::test]
        fn allocate_contiguous_large_alignment<A: Arch>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([2*GIB, 0x1000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions().collect());

            let frame = frame_allocator.allocate_contiguous(Layout::from_size_align(A::GRANULE_SIZE, 1*GIB).unwrap()).unwrap();

            assert!(frame.is_aligned_to(1*GIB));
        }

        #[test_log::test]
        fn allocate_large_alignment<A: Arch>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([2*GIB, 0x1000])
                .finish();

            let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                BootstrapAllocator::new::<A>(machine.memory_regions().collect());

            let blocks = frame_allocator.allocate(Layout::from_size_align(A::GRANULE_SIZE, 1*GIB).unwrap()).unwrap();

            for block in blocks {
                assert!(block.start.is_aligned_to(1*GIB));
            }
        }
    }
}

#[cfg(test)]
mod proptests {
    use core::alloc::Layout;

    use proptest::prelude::*;

    use crate::address_range::AddressRangeExt;
    use crate::arch::Arch;
    use crate::bootstrap::{BootstrapAllocator, DEFAULT_MAX_REGIONS};
    use crate::frame_allocator::FrameAllocator;
    use crate::test_utils::proptest::region_sizes;
    use crate::test_utils::{Machine, MachineBuilder};
    use crate::{GIB, KIB, for_every_arch};

    for_every_arch!(A => {
        proptest! {
            #[test_log::test]
            fn allocate(region_sizes in region_sizes(1..DEFAULT_MAX_REGIONS, 4*KIB, 16*GIB)) {
                let machine: Machine<A> = MachineBuilder::new()
                    .with_memory_regions(region_sizes.clone())
                    .finish();

                let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                    BootstrapAllocator::new::<A>(machine.memory_regions().collect());

                let total_size = region_sizes.iter().sum();

                let res = frame_allocator
                    .allocate(Layout::from_size_align(total_size, A::GRANULE_SIZE).unwrap());
                prop_assert!(res.is_ok(), "failed to allocate {} bytes with alignment {}. capacities left {:?}", total_size, A::GRANULE_SIZE, frame_allocator.capacities());
                let blocks = res.unwrap();

                let blocks: Vec<_> = blocks.collect();

                // assert the total size is what we expect
                let allocated_size: usize = blocks.iter().map(|block| block.len()).sum();
                prop_assert!(allocated_size >= total_size);

                // assert each block is aligned correctly
                for block in blocks.iter() {
                    prop_assert!(block.start.is_aligned_to(A::GRANULE_SIZE));
                }
            }

            #[test_log::test]
            fn allocate_contiguous(region_sizes in region_sizes(1..DEFAULT_MAX_REGIONS, 1*GIB, 16*GIB)) {
                let machine: Machine<A> = MachineBuilder::new()
                    .with_memory_regions(region_sizes.clone())
                    .finish();

                let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                    BootstrapAllocator::new::<A>(machine.memory_regions().collect());

                let total_size = region_sizes.iter().sum();

                for _ in (0..total_size).step_by(1*GIB) {
                    let res = frame_allocator.allocate_contiguous(Layout::from_size_align(1*GIB, A::GRANULE_SIZE).unwrap());
                    prop_assert!(res.is_ok());
                    let base = res.unwrap();
                    prop_assert!(base.is_aligned_to(A::GRANULE_SIZE));
                }
            }

            #[test_log::test]
            fn allocate_contiguous_alignments(region_sizes in region_sizes(1..DEFAULT_MAX_REGIONS, 1*GIB, 16*GIB), alignment_pot in 1..30) {
                let machine: Machine<A> = MachineBuilder::new()
                    .with_memory_regions(region_sizes.clone())
                    .finish();

                let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                    BootstrapAllocator::new::<A>(machine.memory_regions().collect());

                let alignment = 1usize << alignment_pot;

                let res = frame_allocator.allocate_contiguous(Layout::from_size_align(A::GRANULE_SIZE, alignment).unwrap());
                prop_assert!(res.is_ok());
                let base = res.unwrap();

                prop_assert!(base.is_aligned_to(alignment));
            }

            #[test_log::test]
            fn allocate_alignments(region_sizes in region_sizes(1..DEFAULT_MAX_REGIONS, 1*GIB, 16*GIB), alignment_pot in 1..30) {
                let machine: Machine<A> = MachineBuilder::new()
                    .with_memory_regions(region_sizes.clone())
                    .finish();

                let frame_allocator: BootstrapAllocator<parking_lot::RawMutex> =
                    BootstrapAllocator::new::<A>(machine.memory_regions().collect());

                let alignment = 1usize << alignment_pot;

                let res = frame_allocator.allocate(Layout::from_size_align(A::GRANULE_SIZE, alignment).unwrap());
                prop_assert!(res.is_ok());
                let blocks = res.unwrap();

                for block in blocks {
                    prop_assert!(block.start.is_aligned_to(alignment));
                }
            }
        }
    });
}
