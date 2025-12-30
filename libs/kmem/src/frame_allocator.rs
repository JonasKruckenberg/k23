use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::{cmp, fmt};

use fallible_iterator::FallibleIterator;

use crate::arch::Arch;
use crate::physmap::PhysMap;
use crate::{AddressRangeExt, PhysicalAddress};

/// The `AllocError` error indicates a frame allocation failure that may be due
/// to resource exhaustion or to something wrong when combining the given input
/// arguments with this allocator.
#[derive(Debug, Copy, Clone)]
pub struct AllocError;

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("physical memory allocation failed")
    }
}

impl core::error::Error for AllocError {}

/// An implementation of Allocator can allocate and deallocate physical memory blocks described via [`Layout`].
///
/// `Allocator` is designed to be implemented on ZSTs, references, or smart pointers. An allocator for
/// `MyAlloc([u8; N])` cannot be moved, without updating the pointers to the allocated memory.
///
/// # Safety
///
/// Memory blocks that are currently allocated by an allocator, must point to valid memory, and
/// retain their validity until either:
///
/// - the memory block is deallocated, or
/// - the allocator is dropped.
///
/// Copying, cloning, or moving the allocator must not invalidate memory blocks returned from it.
/// A copied or cloned allocator must behave like the original allocator.
///
/// A memory block which is currently allocated may be passed to any method of the allocator that
/// accepts such an argument.
pub unsafe trait FrameAllocator {
    fn allocate(&self, layout: Layout) -> FrameIter<'_, Self>
    where
        Self: Sized,
    {
        FrameIter {
            alloc: self,
            remaining: layout.size(),
            alignment: layout.align(),
        }
    }

    fn allocate_zeroed<'a, A: Arch>(
        &self,
        layout: Layout,
        physmap: &'a PhysMap,
        arch: &'a A,
    ) -> FrameIterZeroed<'_, 'a, Self, A>
    where
        Self: Sized,
    {
        FrameIterZeroed {
            inner: self.allocate(layout),
            physmap,
            arch,
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
    fn allocate_contiguous_zeroed(
        &self,
        layout: Layout,
        physmap: &PhysMap,
        arch: &impl Arch,
    ) -> Result<PhysicalAddress, AllocError> {
        let frame = self.allocate_contiguous(layout)?;

        let page = physmap.phys_to_virt(frame);

        // Safety: the address is properly aligned (at least page aligned) and is either valid to
        // access through the physical memory map or because we're in bootstrapping still and phys==virt
        unsafe {
            arch.write_bytes(page, 0, layout.size());
        }

        Ok(frame)
    }

    /// Deallocates the block of memory referenced by `block`.
    ///
    /// # Safety
    ///
    /// 1. `block` must denote a block of frames *currently allocated* via this allocator, and
    /// 2. `layout` must *fit* that block of frames.
    unsafe fn deallocate(&self, block: PhysicalAddress, layout: Layout);

    /// Creates a "by reference" adapter for this instance of `FrameAllocator`.
    ///
    /// The returned adapter also implements `FrameAllocator` and will simply borrow this.
    #[inline(always)]
    fn by_ref(&self) -> &Self
    where
        Self: Sized,
    {
        self
    }

    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>);
}

// Safety: we just forward to the inner implementation
unsafe impl<F> FrameAllocator for &F
where
    F: FrameAllocator + ?Sized,
{
    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        (**self).allocate_contiguous(layout)
    }

    // fn allocate_contiguous_zeroed(&self, layout: Layout, arch: &impl Arch) -> Result<PhysicalAddress, AllocError> {
    //     (**self).allocate_contiguous_zeroed(layout, arch)
    // }

    unsafe fn deallocate(&self, block: PhysicalAddress, layout: Layout) {
        // Safety: ensured by caller
        unsafe { (**self).deallocate(block, layout) }
    }

    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>) {
        (**self).size_hint()
    }
}

pub struct FrameIter<'alloc, F: ?Sized> {
    alloc: &'alloc F,
    remaining: usize,
    alignment: usize,
}

impl<F: FrameAllocator> FallibleIterator for FrameIter<'_, F> {
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
        let layout = Layout::from_size_align(alloc_size.get(), self.alignment).unwrap();

        let addr = self.alloc.allocate_contiguous(layout)?;

        self.remaining -= requested_size.get();

        Ok(Some(Range::from_start_len(addr, requested_size.get())))
    }
}

pub struct FrameIterZeroed<'alloc, 'a, F: ?Sized, A: Arch> {
    inner: FrameIter<'alloc, F>,
    physmap: &'a PhysMap,
    arch: &'a A,
}

impl<F: FrameAllocator, A: Arch> FallibleIterator for FrameIterZeroed<'_, '_, F, A> {
    type Item = Range<PhysicalAddress>;
    type Error = AllocError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let Some(range) = self.inner.next()? else {
            return Ok(None);
        };

        let virt = self.physmap.phys_to_virt_range(range.clone());

        // Safety: we just allocated the frame
        unsafe {
            self.arch.write_bytes(virt.start, 0, virt.len());
        }

        Ok(Some(range))
    }
}
