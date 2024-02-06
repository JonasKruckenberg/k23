use crate::paging::VirtualAddress;
use core::alloc::AllocError;
use core::ops::Range;
use core::ptr::NonNull;

pub struct Slab<const BLOCK_SIZE: usize> {
    free_frame_list: FreeFrameList,
}

impl<const BLOCK_SIZE: usize> Slab<BLOCK_SIZE> {
    pub unsafe fn new(start: VirtualAddress, heap_size: usize) -> Self {
        let region = start..start.add(heap_size);
        Self {
            free_frame_list: FreeFrameList::new::<BLOCK_SIZE>(region),
        }
    }

    pub fn allocate(&mut self) -> Result<NonNull<u8>, AllocError> {
        self.free_frame_list
            .pop()
            .map(|frame| frame.as_ptr())
            .ok_or(AllocError)
    }

    pub fn deallocate(&mut self, ptr: NonNull<u8>) {
        let ptr = ptr.as_ptr() as *mut FreeFrame;
        unsafe {
            self.free_frame_list.push(&mut *ptr);
        }
    }
}

struct FreeFrameList {
    len: usize,
    head: Option<&'static mut FreeFrame>,
}

impl FreeFrameList {
    unsafe fn new<const BLOCK_SIZE: usize>(region: Range<VirtualAddress>) -> Self {
        let mut new_list = Self { len: 0, head: None };

        let num_of_frames = (region.end.as_raw() - region.start.as_raw()) / BLOCK_SIZE;

        for i in (0..num_of_frames).rev() {
            let new_frame = region.start.add(i * BLOCK_SIZE).as_raw() as *mut FreeFrame;
            new_list.push(&mut *new_frame);
        }

        new_list
    }

    pub fn pop(&mut self) -> Option<&mut FreeFrame> {
        self.head.take().map(|block| {
            self.head = block.next.take();
            self.len -= 1;
            block
        })
    }

    fn push(&mut self, free_block: &'static mut FreeFrame) {
        free_block.next = self.head.take();
        self.len += 1;
        self.head = Some(free_block);
    }
}

struct FreeFrame {
    next: Option<&'static mut FreeFrame>,
}

impl FreeFrame {
    fn as_ptr(&mut self) -> NonNull<u8> {
        let ptr = self as *mut _ as *mut u8; // yuck

        unsafe { NonNull::new_unchecked(ptr) }
    }
}
