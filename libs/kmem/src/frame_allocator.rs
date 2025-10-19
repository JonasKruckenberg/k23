// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::error::Error;
use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::{cmp, fmt, ptr};
use core::ops::Range;
use fallible_iterator::FallibleIterator;

use crate::arch::Arch;
use crate::{AddressRangeExt, PhysicalAddress};

/// The `AllocError` error indicates a frame allocation failure that may be due
/// to resource exhaustion or to something wrong when combining the given input
/// arguments with this allocator.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct AllocError;

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("memory allocation failed")
    }
}

impl Error for AllocError {}

pub unsafe trait FrameAllocator<A: Arch> {
    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>);

    /// Attempts to allocate memory meeting the size and alignment guarantees
    /// of `layout`.
    ///
    /// Unlike [`allocate`][Self::allocate] the allocations are **not** required to be contiguous
    /// and might be split into multiple chunks.
    ///
    /// - the first yielded chunk of the allocation will meet the alignment requirements
    /// - subsequent chunks may not
    /// - chunks will never be smaller than A::PAGE_SIZE
    ///
    /// The total returned block may have a larger size than specified by `layout.size()`, and may
    /// or may not have its contents initialized.
    ///
    /// - Intention: when physical memory must be mapped to back virtual memory, contiguousness is
    /// not important and relaxed allocation can help allocators use memory more efficiently
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates that either memory is exhausted or `layout` does not meet
    /// allocator's size or alignment constraints. You can check [`Self::max_alignment_hint`] for
    /// the largest alignment possibly supported by this allocator.
    fn allocate(&self, layout: Layout) -> FrameIter<'_, Self, A>
    where
        Self: Sized,
    {
        FrameIter {
            alloc: self,
            remaining: layout.size(),
            alignment: layout.align(),
            _arch: PhantomData,
        }
    }

    /// Behaves like [`allocate`][Self::allocate], but also ensures that the returned memory is
    /// zero-initialized.
    ///
    /// Note that only `layout.size()`-bytes are guaranteed to be zeroed.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates that either memory is exhausted or `layout` does not meet
    /// allocator's size or alignment constraints. You can check [`Self::max_alignment_hint`] for
    /// the largest alignment possibly supported by this allocator.
    fn allocate_zeroed(&self, layout: Layout) -> FrameIterZeroed<'_, Self, A>
    where
        Self: Sized,
    {
        FrameIterZeroed {
            inner: self.allocate(layout),
        }
    }

    /// Attempts to allocate a contiguous block of physical memory.
    ///
    /// On success, returns a [`PhysicalAddress`] meeting the size and alignment guarantees
    /// of `layout`.
    ///
    /// The returned block may have a larger size than specified by `layout.size()`, and may or may
    /// not have its contents initialized.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates that either memory is exhausted or `layout` does not meet
    /// allocator's size or alignment constraints. You can check [`Self::max_alignment_hint`] for
    /// the largest alignment possibly supported by this allocator.
    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError>;

    /// Behaves like [`allocate`][Self::allocate], but also ensures that the returned memory is
    /// zero-initialized.
    ///
    /// Note that only `layout.size()`-bytes are guaranteed to be zeroed.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates that either memory is exhausted or `layout` does not meet
    /// allocator's size or alignment constraints. You can check [`Self::max_alignment_hint`] for
    /// the largest alignment possibly supported by this allocator.
    fn allocate_contiguous_zeroed(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let addr = self.allocate_contiguous(layout)?;

        // Safety: we just allocated the frame
        unsafe {
            ptr::write_bytes::<u8>(A::phys_to_virt(addr).as_mut_ptr().cast::<u8>(), 0, layout.size());
        }

        Ok(addr)
    }

    /// Deallocates the block of memory referenced by `block`.
    ///
    /// # Safety
    ///
    /// 1. `block` must denote a block of frames *currently allocated* via this allocator, and
    /// 2. `layout` must *fit* that block of frames.
    unsafe fn deallocate(&self, block: PhysicalAddress, layout: Layout);

    /// Creates a "by reference" adapter for this instance of `Allocator`.
    ///
    /// The returned adapter also implements `Allocator` and will simply borrow this.
    #[inline(always)]
    fn by_ref(&self) -> &Self
    where
        Self: Sized,
    {
        self
    }
}

unsafe impl<A, F> FrameAllocator<A> for &F
where
    A: Arch,
    F: FrameAllocator<A> + ?Sized,
{
    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>) {
        (**self).size_hint()
    }

    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        (**self).allocate_contiguous(layout)
    }

    unsafe fn deallocate(&self, block: PhysicalAddress, layout: Layout) {
        unsafe { (**self).deallocate(block, layout) }
    }
}

pub struct FrameIter<'alloc, F, A> {
    alloc: &'alloc F,
    remaining: usize,
    alignment: usize,
    _arch: PhantomData<A>,
}

impl<A: Arch, F: FrameAllocator<A>> FallibleIterator for FrameIter<'_, F, A> {
    type Item = Range<PhysicalAddress>;
    type Error = AllocError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let Some(remaining) = NonZeroUsize::new(self.remaining) else {
            return Ok(None);
        };

        let (min_size, max_size) = self.alloc.size_hint();

        let requested_size = cmp::min(remaining, max_size.unwrap_or(NonZeroUsize::MAX));
        let alloc_size = cmp::max(requested_size, min_size);

        log::trace!(
            "requested_size={requested_size:?} alloc_size={alloc_size:?} align={:?}",
            self.alignment
        );
        let layout = unsafe { Layout::from_size_align_unchecked(alloc_size.get(), self.alignment) };

        let addr = self.alloc.allocate_contiguous(layout)?;

        self.remaining -= requested_size.get();

        Ok(Some(Range::from_start_len(addr, requested_size.get())))
    }
}

pub struct FrameIterZeroed<'alloc, F, A> {
    inner: FrameIter<'alloc, F, A>,
}

impl<A: Arch, F: FrameAllocator<A>> FallibleIterator for FrameIterZeroed<'_, F, A> {
    type Item = Range<PhysicalAddress>;
    type Error = AllocError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let Some(chunk) = self.inner.next()? else {
            return Ok(None);
        };

        // Safety: we just allocated the frame
        unsafe {
            ptr::write_bytes::<u8>(A::phys_to_virt(chunk.start).as_mut_ptr().cast::<u8>(), 0, chunk.len());
        }

        Ok(Some(chunk))
    }
}

#[cfg(test)]
mod tests {
    use core::alloc::Layout;
    use core::num::NonZeroUsize;
    use core::slice;

    use fallible_iterator::FallibleIterator;
    use test_log::test;

    use crate::arch::Arch as _;
    use crate::test_utils::TestFrameAllocator;
    use crate::{AddressRangeExt, FrameAllocator};

    type Arch = crate::test_utils::TestArch<crate::arch::riscv64::RiscV64Sv39>;

    #[test]
    fn allocate_contiguous_returns_single_aligned_block() {
        let allocator = TestFrameAllocator::<Arch>::new();

        let (layout, _) = Arch::PAGE_LAYOUT.repeat(3).unwrap();

        let addr = allocator.allocate_contiguous(layout).unwrap();
        assert!(addr.is_aligned_to(layout.align()));
    }

    #[test]
    fn allocate_contiguous_zeroed_returns_single_aligned_block() {
        let allocator = TestFrameAllocator::<Arch>::new();

        let (layout, _) = Arch::PAGE_LAYOUT.repeat(3).unwrap();

        let addr = allocator.allocate_contiguous_zeroed(layout).unwrap();
        assert!(addr.is_aligned_to(layout.align()));
    }

    #[test]
    fn allocate_contiguous_zeroed_returns_zeroed_block() {
        let allocator = TestFrameAllocator::<Arch>::new();

        let (layout, _) = Arch::PAGE_LAYOUT.repeat(3).unwrap();

        let addr = allocator.allocate_contiguous_zeroed(layout).unwrap();

        let memory =
            unsafe { slice::from_raw_parts(Arch::phys_to_virt(addr).as_ptr().cast::<u8>(), layout.size()) };
        assert!(memory.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn allocate_respects_alignment() {
        let allocator = TestFrameAllocator::<Arch>::new();
        let layout = Layout::from_size_align(4096, 8192).unwrap();

        let mut iter = allocator.allocate(layout);
        let chunk = iter.next().unwrap().unwrap();

        assert!(chunk.start.is_aligned_to(layout.align()));
    }

    #[test]
    fn allocate_single_page_returns_aligned_address() {
        let allocator = TestFrameAllocator::<Arch>::new();
        let layout = Arch::PAGE_LAYOUT;

        let mut iter = allocator.allocate(layout);

        let chunk = iter.next().unwrap().unwrap();

        assert!(chunk.start.is_aligned_to(layout.align()));
        assert_eq!(chunk.len(), layout.size());
        assert!(iter.next().unwrap().is_none());
    }

    /// Assert that `allocate` correctly splits allocations into chunks.
    ///
    /// The underlying allocator in this case can only allocate 4096 byte chunks; We therefore expect
    /// `allocate` to return 3 chunks of 4096 bytes (because we request 3 pages).
    #[test]
    fn allocate_multiple_pages() {
        let alloc = TestFrameAllocator::<Arch>::new()
            .with_max_block_size(NonZeroUsize::new(Arch::PAGE_SIZE).unwrap());

        let (layout, _) = Arch::PAGE_LAYOUT.repeat(3).unwrap();

        let mut count = 0;
        for chunk in alloc.allocate(layout).unwrap() {
            assert!(chunk.start.is_aligned_to(layout.align()));
            assert_eq!(chunk.len(), Arch::PAGE_SIZE);

            count += 1;
        }
        assert_eq!(count, 3);

        let allocations = alloc.allocations();
        assert_eq!(allocations.len(), 3);
        for (_, (block, _)) in allocations.iter() {
            assert_eq!(block.len(), Arch::PAGE_SIZE);
        }
    }

    /// Assert that `allocate` correctly splits allocations into chunks.
    ///
    /// The underlying allocator in this case can only allocate 8192 byte chunks; We therefore expect
    /// `allocate` to return 3 chunks of 8192 bytes with a trailing 4096 chunk (because we request 7 pages)
    ///
    /// The underlying allocator will still hold 4 chunks of 8192 bytes because of the minimum chunk size.
    #[test]
    fn allocate_multiple_pages_multi_page_chunks() {
        // Allocator that forces allocations to occur in multiple-of-8192-byte chunks.
        let alloc = TestFrameAllocator::<Arch>::new()
            .with_max_block_size(NonZeroUsize::new(2 * Arch::PAGE_SIZE).unwrap())
            .with_min_block_size(NonZeroUsize::new(2 * Arch::PAGE_SIZE).unwrap());

        let (layout, _) = Arch::PAGE_LAYOUT.repeat(7).unwrap();

        let mut chunks = 0;
        let mut total = 0;
        for chunk in alloc.allocate(layout).unwrap() {
            assert!(chunk.start.is_aligned_to(layout.align()));
            // must never be smaller than a page
            assert!(chunk.len() >= Arch::PAGE_SIZE);
            // must never be larger than the alloc chunk size we set above
            assert!(chunk.len() <= 2 * Arch::PAGE_SIZE);

            chunks += 1;
            total += chunk.len();
        }
        assert_eq!(chunks, 4);
        assert_eq!(total, layout.size());

        let allocations = alloc.allocations();
        assert_eq!(allocations.len(), 4);
        for (_, (block, _)) in allocations.iter() {
            assert_eq!(block.len(), 2 * Arch::PAGE_SIZE);
        }
    }

    #[test]
    fn allocate_zeroed_single_page_contains_zeros() {
        let allocator = TestFrameAllocator::<Arch>::new();

        let mut iter = allocator.allocate_zeroed(Arch::PAGE_LAYOUT);

        let chunk = iter.next().unwrap().unwrap();

        let memory =
            unsafe { slice::from_raw_parts(Arch::phys_to_virt(chunk.start).as_ptr().cast::<u8>(), chunk.len()) };
        assert!(memory.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn allocate_zeroed_multiple_pages_all_contain_zeros() {
        let alloc = TestFrameAllocator::<Arch>::new();

        let (layout, _) = Arch::PAGE_LAYOUT.repeat(3).unwrap();
        assert!(layout.align().is_multiple_of(4096));
        assert!(layout.size().is_multiple_of(4096));

        for chunk in alloc.allocate_zeroed(layout).unwrap() {
            let memory =
                unsafe { slice::from_raw_parts(Arch::phys_to_virt(chunk.start).as_ptr().cast::<u8>(), chunk.len()) };
            assert!(memory.iter().all(|&byte| byte == 0));
        }
    }
}
