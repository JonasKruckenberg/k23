//! In-tree vendored copy of the [`linked-list-allocator`](https://crates.io/crates/linked_list_allocator) (MIT/Apache-2.0) crate,
//! with some modifications to better integrate it with k23.

#![no_std]
#![feature(allocator_api, alloc_layout_extra)]

use core::alloc::GlobalAlloc;
use core::alloc::Layout;
use core::alloc::{AllocError, Allocator};
use core::mem::MaybeUninit;
use core::ops::{Deref, Range};
use core::ptr::NonNull;
use hole::HoleList;
use kmm::AddressRangeExt;
use sync::Mutex;

pub mod hole;

/// A fixed size heap backed by a linked list of free memory blocks.
pub struct Heap {
    used: usize,
    holes: HoleList,
}

// #[cfg(fuzzing)]
// impl Heap {
//     pub fn debug(&mut self) {
//         println!(
//             "bottom: {:?}, top: {:?}, size: {}, pending: {}",
//             self.bottom(),
//             self.top(),
//             self.size(),
//             self.holes.first.size,
//         );
//         self.holes.debug();
//     }
// }

unsafe impl Send for Heap {}

impl Heap {
    /// Creates an empty heap. All allocate calls will return `None`.
    pub const fn empty() -> Heap {
        Heap {
            used: 0,
            holes: HoleList::empty(),
        }
    }

    /// Initializes an empty heap
    ///
    /// The range is automatically aligned, so the [`bottom()`][Self::bottom]
    /// method might return a pointer that is larger than `heap_bottom` after construction.
    ///
    /// The given range must be large enough to store the required
    /// metadata, otherwise this function will panic. Depending on the
    /// alignment of the `hole_addr` pointer, the minimum size is between
    /// `2 * size_of::<usize>` and `3 * size_of::<usize>`.
    ///
    /// The usable size for allocations will be truncated to the nearest
    /// alignment of `align_of::<usize>`. Any extra bytes left at the end
    /// will be reclaimed once sufficient additional space is given to
    /// [`extend`][Heap::extend].
    /// # Safety
    ///
    /// This function must be called at most once and must only be used on an
    /// empty heap.
    ///
    /// The bottom address must be valid and the memory in the range must not be used for
    /// anything else. This function is unsafe because it can cause undefined behavior
    /// if the given address is invalid.
    ///
    /// The provided memory range must be valid for the `'static` lifetime.
    pub unsafe fn init_from_phys_range(&mut self, range: Range<kmm::PhysicalAddress>) {
        self.used = 0;
        self.holes = HoleList::new(range.start.as_raw() as *mut u8, range.size());
    }

    /// Initializes an empty heap
    ///
    /// The range is automatically aligned, so the [`bottom()`][Self::bottom]
    /// method might return a pointer that is larger than `heap_bottom` after construction.
    ///
    /// The given range must be large enough to store the required
    /// metadata, otherwise this function will panic. Depending on the
    /// alignment of the `hole_addr` pointer, the minimum size is between
    /// `2 * size_of::<usize>` and `3 * size_of::<usize>`.
    ///
    /// The usable size for allocations will be truncated to the nearest
    /// alignment of `align_of::<usize>`. Any extra bytes left at the end
    /// will be reclaimed once sufficient additional space is given to
    /// [`extend`][Heap::extend].
    /// # Safety
    ///
    /// This function must be called at most once and must only be used on an
    /// empty heap.
    ///
    /// The bottom address must be valid and the memory in the range must not be used for
    /// anything else. This function is unsafe because it can cause undefined behavior
    /// if the given address is invalid.
    ///
    /// The provided memory range must be valid for the `'static` lifetime.
    pub unsafe fn init_from_virt_range(&mut self, range: Range<kmm::VirtualAddress>) {
        self.used = 0;
        self.holes = HoleList::new(range.start.as_raw() as *mut u8, range.size());
    }

    /// Allocates a chunk of the given size with the given alignment. Returns a pointer to the
    /// beginning of that chunk if it was successful. Else it returns `None`.
    /// This function scans the list of free memory blocks and uses the first block that is big
    /// enough. The runtime is in O(n) where n is the number of free blocks, but it should be
    /// reasonably fast for small allocations.
    //
    // NOTE: We could probably replace this with an `Option` instead of a `Result` in a later
    // release to remove this clippy warning
    #[allow(clippy::result_unit_err)]
    pub fn allocate_first_fit(&mut self, layout: Layout) -> Result<NonNull<u8>, ()> {
        match self.holes.allocate_first_fit(layout) {
            Ok((ptr, aligned_layout)) => {
                self.used += aligned_layout.size();
                Ok(ptr)
            }
            Err(err) => Err(err),
        }
    }

    /// Frees the given allocation. `ptr` must be a pointer returned
    /// by a call to the `allocate_first_fit` function with identical size and alignment.
    ///
    /// This function walks the list of free memory blocks and inserts the freed block at the
    /// correct place. If the freed block is adjacent to another free block, the blocks are merged
    /// again. This operation is in `O(n)` since the list needs to be sorted by address.
    ///
    /// # Safety
    ///
    /// `ptr` must be a pointer returned by a call to the [`allocate_first_fit`] function with
    /// identical layout. Undefined behavior may occur for invalid arguments.
    pub unsafe fn deallocate(&mut self, ptr: NonNull<u8>, layout: Layout) {
        self.used -= self.holes.deallocate(ptr, layout).size();
    }

    /// Returns the bottom address of the heap.
    ///
    /// The bottom pointer is automatically aligned, so the returned pointer
    /// might be larger than the bottom pointer used for initialization.
    pub fn bottom(&self) -> *mut u8 {
        self.holes.bottom
    }

    /// Returns the size of the heap.
    ///
    /// This is the size the heap is using for allocations, not necessarily the
    /// total amount of bytes given to the heap. To determine the exact memory
    /// boundaries, use [`bottom`][Self::bottom] and [`top`][Self::top].
    pub fn size(&self) -> usize {
        unsafe { self.holes.top.offset_from(self.holes.bottom) as usize }
    }

    /// Return the top address of the heap.
    ///
    /// Note: The heap may choose to not use bytes at the end for allocations
    /// until there is enough room for metadata, but it still retains ownership
    /// over memory from [`bottom`][Self::bottom] to the address returned.
    pub fn top(&self) -> *mut u8 {
        unsafe { self.holes.top.add(self.holes.pending_extend as usize) }
    }

    /// Returns the size of the used part of the heap
    pub fn used(&self) -> usize {
        self.used
    }

    /// Returns the size of the free part of the heap
    pub fn free(&self) -> usize {
        self.size() - self.used
    }

    /// Extends the size of the heap by creating a new hole at the end.
    ///
    /// Small extensions are not guaranteed to grow the usable size of
    /// the heap. In order to grow the Heap most effectively, extend by
    /// at least `2 * size_of::<usize>`, keeping the amount a multiple of
    /// `size_of::<usize>`.
    ///
    /// Calling this method on an uninitialized Heap will panic.
    ///
    /// # Safety
    ///
    /// The amount of data given in `by` MUST exist directly after the original
    /// range of data provided when constructing the [Heap]. The additional data
    /// must have the same lifetime of the original range of data.
    ///
    /// Even if this operation doesn't increase the [usable size][`Self::size`]
    /// by exactly `by` bytes, those bytes are still owned by the Heap for
    /// later use.
    pub unsafe fn extend(&mut self, by: usize) {
        self.holes.extend(by);
    }
}

unsafe impl Allocator for LockedHeap {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if layout.size() == 0 {
            return Ok(NonNull::slice_from_raw_parts(layout.dangling(), 0));
        }
        match self.0.lock().allocate_first_fit(layout) {
            Ok(ptr) => Ok(NonNull::slice_from_raw_parts(ptr, layout.size())),
            Err(()) => Err(AllocError),
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() != 0 {
            self.0.lock().deallocate(ptr, layout);
        }
    }
}

pub struct LockedHeap(Mutex<Heap>);

impl LockedHeap {
    pub const fn empty() -> LockedHeap {
        LockedHeap(Mutex::new(Heap::empty()))
    }

    /// Creates a new heap with the given `bottom` and `size`.
    ///
    /// The `heap_bottom` pointer is automatically aligned, so the [`bottom()`][Heap::bottom]
    /// method might return a pointer that is larger than `heap_bottom` after construction.
    ///
    /// The given `heap_size` must be large enough to store the required
    /// metadata, otherwise this function will panic. Depending on the
    /// alignment of the `hole_addr` pointer, the minimum size is between
    /// `2 * size_of::<usize>` and `3 * size_of::<usize>`.
    ///
    /// # Safety
    ///
    /// The bottom address must be valid and the memory in the
    /// `[heap_bottom, heap_bottom + heap_size)` range must not be used for anything else.
    /// This function is unsafe because it can cause undefined behavior if the given address
    /// is invalid.
    ///
    /// The provided memory range must be valid for the `'static` lifetime.
    pub unsafe fn new(heap_bottom: *mut u8, heap_size: usize) -> LockedHeap {
        LockedHeap(Mutex::new(Heap {
            used: 0,
            holes: HoleList::new(heap_bottom, heap_size),
        }))
    }
}

impl Deref for LockedHeap {
    type Target = Mutex<Heap>;

    fn deref(&self) -> &Mutex<Heap> {
        &self.0
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0
            .lock()
            .allocate_first_fit(layout)
            .ok()
            .map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.0
            .lock()
            .deallocate(NonNull::new_unchecked(ptr), layout)
    }
}
