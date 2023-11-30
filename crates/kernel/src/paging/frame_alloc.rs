use crate::arch::PAGE_SIZE;
use crate::paging::PhysicalAddress;
use crate::Error;
use core::ops::Range;

pub struct FrameAllocator {
    free_frame_list: FreeFrameList,
}

impl FrameAllocator {
    pub unsafe fn new(regions: &[Range<PhysicalAddress>]) -> Self {
        Self {
            free_frame_list: FreeFrameList::new(regions),
        }
    }

    pub fn allocate_frame(&mut self) -> crate::Result<PhysicalAddress> {
        self.free_frame_list
            .pop()
            .map(|frame| frame.as_ptr())
            .ok_or(crate::Error::OutOfMemory)
    }

    pub fn allocate_frames(&mut self, n: usize) -> crate::Result<PhysicalAddress> {
        let mut current = self.free_frame_list.head.as_ref();
        let mut first_frame = current;
        let mut remaining = n;

        while let Some(frame) = current {
            if remaining == 0 {
                return Ok(first_frame.unwrap().as_ptr());
            } else if let Some(next) = frame.next.as_ref() {
                // check if the next frame is consecutive
                if next.as_ptr().as_raw() == frame.as_ptr().as_raw() + PAGE_SIZE {
                    remaining -= 1;
                    current = Some(next);
                } else {
                    // we found a free frame but its in a different region
                    // so we need to start over
                    remaining = n;
                    first_frame = current;
                }
            }
        }

        // we didn't find enough free frames
        Err(Error::OutOfMemory)
    }

    pub fn deallocate_frame(&mut self, phys: PhysicalAddress) {
        let ptr = phys.0 as *mut FreeFrame;
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
    unsafe fn new(regions: &[Range<PhysicalAddress>]) -> Self {
        let mut new_list = Self { len: 0, head: None };

        for region in regions {
            let num_of_frames = (region.end.0 - region.start.0) / PAGE_SIZE;

            for i in (0..num_of_frames).rev() {
                let new_frame = (region.start.0 + i * PAGE_SIZE) as *mut FreeFrame;
                new_list.push(&mut *new_frame);
            }
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
    fn as_ptr(&self) -> PhysicalAddress {
        let ptr = self as *const _ as usize;
        PhysicalAddress(ptr)
    }
}
