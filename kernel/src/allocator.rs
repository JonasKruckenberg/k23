use core::alloc::{GlobalAlloc, Layout};

#[global_allocator]
static KERNEL_ALLOCATOR: Heap = Heap;

struct Heap;
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, _layout: Layout) -> *mut u8 {
        todo!()
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        todo!()
    }
}
