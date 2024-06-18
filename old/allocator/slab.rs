use core::alloc::AllocError;
use core::ptr::NonNull;
use vmm::VirtualAddress;

#[derive(Debug)]
#[allow(unused)]
pub struct SlabUsage {
    pub size: usize,
    pub used: usize,
    pub total: usize,
}

pub struct Slab<const BLOCK_SIZE: usize> {
    free_block_list: FreeBlockList<BLOCK_SIZE>,
    blocks_total: usize,
}

struct FreeBlockList<const BLOCK_SIZE: usize> {
    len: usize,
    head: Option<&'static mut FreeBlock>,
}

struct FreeBlock {
    next: Option<&'static mut FreeBlock>,
}

impl<const BLOCK_SIZE: usize> Slab<BLOCK_SIZE> {
    pub unsafe fn new(start: VirtualAddress, size: usize) -> Self {
        log::trace!("initializing {BLOCK_SIZE} byte slab at {start:?} with len {size} bytes");
        Self {
            free_block_list: FreeBlockList::new(start, size / BLOCK_SIZE),
            blocks_total: size / BLOCK_SIZE,
        }
    }

    pub fn usage(&self) -> SlabUsage {
        SlabUsage {
            size: BLOCK_SIZE,
            total: self.blocks_total,
            used: self.blocks_total - self.free_block_list.len,
        }
    }

    pub fn allocate(&mut self) -> Result<NonNull<u8>, AllocError> {
        match self.free_block_list.pop() {
            Some(block) => Ok(block.as_ptr()),
            None => Err(AllocError),
        }
    }

    pub fn deallocate(&mut self, ptr: NonNull<u8>) {
        let ptr = ptr.as_ptr() as *mut FreeBlock;
        unsafe {
            self.free_block_list.push(&mut *ptr);
        }
    }
}

impl<const BLOCK_SIZE: usize> FreeBlockList<BLOCK_SIZE> {
    unsafe fn new(start: VirtualAddress, num_blocks: usize) -> Self {
        let mut new_list = Self { len: 0, head: None };

        for i in (0..num_blocks).rev() {
            let new_frame = start.add(i * BLOCK_SIZE).as_raw() as *mut FreeBlock;
            new_list.push(&mut *new_frame);
        }

        new_list
    }

    pub fn pop(&mut self) -> Option<&mut FreeBlock> {
        self.head.take().map(|block| {
            self.head = block.next.take();
            self.len -= 1;
            block
        })
    }

    fn push(&mut self, free_block: &'static mut FreeBlock) {
        free_block.next = self.head.take();
        self.len += 1;
        self.head = Some(free_block);
    }
}

impl FreeBlock {
    fn as_ptr(&self) -> NonNull<u8> {
        let ptr = self as *const _ as *mut u8;
        unsafe { NonNull::new_unchecked(ptr) }
    }
}
