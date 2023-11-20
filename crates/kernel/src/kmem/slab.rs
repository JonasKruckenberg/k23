use core::alloc::AllocError;
use core::ptr::NonNull;

/// A simple fixed-size slab allocator.
///
/// This internally uses a linked list of free blocks to keep track of allocations.
pub struct Slab<const BLOCK_SIZE: usize> {
    free_block_list: FreeBlockList<BLOCK_SIZE>,
}

struct FreeBlockList<const BLOCK_SIZE: usize> {
    len: usize,
    head: Option<&'static mut FreeBlock<BLOCK_SIZE>>,
}

struct FreeBlock<const BLOCK_SIZE: usize> {
    next: Option<&'static mut FreeBlock<BLOCK_SIZE>>,
}

impl<const BLOCK_SIZE: usize> Slab<BLOCK_SIZE> {
    pub unsafe fn new(start_addr: usize, slab_size: usize) -> Slab<BLOCK_SIZE> {
        let num_of_blocks = slab_size / BLOCK_SIZE;
        Slab {
            free_block_list: FreeBlockList::new(start_addr, BLOCK_SIZE, num_of_blocks),
        }
    }

    pub fn allocate(&mut self) -> Result<NonNull<u8>, AllocError> {
        match self.free_block_list.pop() {
            Some(block) => Ok(block.as_ptr()),
            None => Err(AllocError),
        }
    }

    pub fn deallocate(&mut self, ptr: NonNull<u8>) {
        let ptr = ptr.as_ptr() as *mut FreeBlock<BLOCK_SIZE>;
        unsafe {
            self.free_block_list.push(&mut *ptr);
        }
    }
}

impl<const BLOCK_SIZE: usize> FreeBlockList<BLOCK_SIZE> {
    unsafe fn new(
        start_addr: usize,
        block_size: usize,
        num_of_blocks: usize,
    ) -> FreeBlockList<BLOCK_SIZE> {
        let mut new_list = FreeBlockList { len: 0, head: None };

        for i in (0..num_of_blocks).rev() {
            let new_block = (start_addr + i * block_size) as *mut FreeBlock<BLOCK_SIZE>;
            new_list.push(&mut *new_block);
        }
        new_list
    }

    pub fn pop(&mut self) -> Option<&mut FreeBlock<BLOCK_SIZE>> {
        self.head.take().map(|block| {
            self.head = block.next.take();
            self.len -= 1;
            block
        })
    }

    fn push(&mut self, free_block: &'static mut FreeBlock<BLOCK_SIZE>) {
        free_block.next = self.head.take();
        self.len += 1;
        self.head = Some(free_block);
    }
}

impl<const BLOCK_SIZE: usize> FreeBlock<BLOCK_SIZE> {
    fn as_ptr(&self) -> NonNull<u8> {
        let ptr = self as *const _ as *mut u8;
        unsafe { NonNull::new_unchecked(ptr) }
    }
}
