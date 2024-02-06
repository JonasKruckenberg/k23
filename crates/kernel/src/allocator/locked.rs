use crate::allocator::heap::Heap;
use crate::paging::VirtualAddress;
use crate::sync::Mutex;
use core::alloc::{AllocError, Allocator, Layout};
use core::ptr::NonNull;

/// A thread safe wrapper around [`Heap`].
///
/// This type implement the `Allocator` and `GlobalAlloc` traits from the `alloc` crate.
pub struct LockedHeap(Mutex<Option<Heap>>);

impl LockedHeap {
    pub const fn empty() -> Self {
        Self(Mutex::new(None))
    }

    pub unsafe fn init(&self, heap_start_addr: VirtualAddress, heap_size: usize) {
        let heap = Heap::new(heap_start_addr, heap_size);
        self.0.lock().replace(heap);
    }
}

unsafe impl core::alloc::GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.allocate(layout).unwrap().as_ptr() as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.deallocate(NonNull::new(ptr).unwrap(), layout)
    }
}

unsafe impl Allocator for LockedHeap {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if let Some(heap) = self.0.lock().as_mut() {
            heap.allocate(layout)
        } else {
            panic!("Heap not initialized")
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if let Some(heap) = self.0.lock().as_mut() {
            heap.deallocate(ptr, layout)
        } else {
            panic!("Heap not initialized")
        }
    }
}
