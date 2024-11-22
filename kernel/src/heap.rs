use core::alloc;
use core::alloc::Layout;

#[global_allocator]
static HEAP: Heap = Heap;

struct Heap;
unsafe impl alloc::GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        todo!()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        todo!()
    }
}