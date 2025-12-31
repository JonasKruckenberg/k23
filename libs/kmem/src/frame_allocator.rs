use core::alloc::Layout;
use core::fmt;
use core::num::NonZeroUsize;
use core::ops::Range;

use crate::PhysicalAddress;
use crate::arch::Arch;
use crate::physmap::PhysMap;

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
    /// Attempts to allocate physical memory.
    ///
    /// On success, returns an iterator over the allocated chunks of physical memory. The combined
    /// size of all chunks will meet the size required by `Layout` and each chunk will individually
    /// meet the alignment required by `Layout`.
    ///
    /// The returned chunks may have a larger size than specified by `layout.size()`, and may or may
    /// not have its contents initialized.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates that either memory is exhausted or `layout` does not meet
    /// allocator's size or alignment constraints. You can check [`Self::max_alignment_hint`] for
    /// the largest alignment possibly supported by this allocator.
    fn allocate(
        &self,
        layout: Layout,
    ) -> Result<impl Iterator<Item = Range<PhysicalAddress>>, AllocError>;

    /// Attempts to allocate physical memory.
    ///
    /// On success, returns an iterator over the allocated chunks of physical memory. The combined
    /// size of all chunks will meet the size required by `Layout` and each chunk will individually
    /// meet the alignment required by `Layout`.
    ///
    /// The returned chunks may have a larger size than specified by `layout.size()`.
    /// The contents of each chunk will be initialized to zero.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates that either memory is exhausted or `layout` does not meet
    /// allocator's size or alignment constraints. You can check [`Self::max_alignment_hint`] for
    /// the largest alignment possibly supported by this allocator.
    fn allocate_zeroed(
        &self,
        layout: Layout,
        physmap: &PhysMap,
        arch: &impl Arch,
    ) -> Result<impl Iterator<Item = Range<PhysicalAddress>>, AllocError>;

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
    /// The returned block may have a larger size than specified by `layout.size()`.
    /// The contents of the returned block will be initialized to zero.
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
        let phys = self.allocate_contiguous(layout)?;

        let virt = physmap.phys_to_virt(phys);

        // Safety: the address is properly aligned (at least page aligned) and is either valid to
        // access through the physical memory map or because we're in bootstrapping still and phys==virt
        unsafe {
            arch.write_bytes(virt, 0, layout.size());
        }

        Ok(phys)
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
    fn allocate(
        &self,
        layout: Layout,
    ) -> Result<impl Iterator<Item = Range<PhysicalAddress>>, AllocError> {
        (**self).allocate(layout)
    }

    fn allocate_zeroed(
        &self,
        layout: Layout,
        physmap: &PhysMap,
        arch: &impl Arch,
    ) -> Result<impl Iterator<Item = Range<PhysicalAddress>>, AllocError> {
        (**self).allocate_zeroed(layout, physmap, arch)
    }

    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        (**self).allocate_contiguous(layout)
    }

    fn allocate_contiguous_zeroed(
        &self,
        layout: Layout,
        physmap: &PhysMap,
        arch: &impl Arch,
    ) -> Result<PhysicalAddress, AllocError> {
        (**self).allocate_contiguous_zeroed(layout, physmap, arch)
    }

    unsafe fn deallocate(&self, block: PhysicalAddress, layout: Layout) {
        // Safety: ensured by caller
        unsafe { (**self).deallocate(block, layout) }
    }

    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>) {
        (**self).size_hint()
    }
}
