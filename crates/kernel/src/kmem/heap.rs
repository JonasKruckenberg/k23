use crate::kmem::slab::Slab;
use crate::PAGE_SIZE;
use core::alloc::{AllocError, Layout};
use core::ptr::NonNull;

/// A heap allocator.
///
/// This allocator internally uses fixed-size slab allocators speed up allocation of common sizes.
/// All allocations larger than the biggest slab size (4096 currently) are delegated to a linked list allocator.
/// This allocator is not thread safe, see [`LockedHeap`] for a thread safe wrapper.
pub struct Heap {
    slab_64_bytes: Slab<64>,
    slab_128_bytes: Slab<128>,
    slab_256_bytes: Slab<256>,
    slab_512_bytes: Slab<512>,
    slab_1024_bytes: Slab<1024>,
    slab_2048_bytes: Slab<2048>,
    slab_4096_bytes: Slab<4096>,
    // I've looked at the code for the linked list allocator and I decided I don't care enough
    // to implement that horrific pointer madness myself.
    // Feel free to try you hand though, I would appreciate the PR!
    linked_list: linked_list_allocator::Heap,
}

enum AllocatorKind {
    LinkedListAllocator,
    Slab64,
    Slab128,
    Slab256,
    Slab512,
    Slab1024,
    Slab2048,
    Slab4096,
}

impl Heap {
    const NUM_OF_SLABS: usize = 8;
    const MIN_SLAB_SIZE: usize = 4096;
    pub const MIN_SIZE: usize = Self::NUM_OF_SLABS * Self::MIN_SLAB_SIZE;

    pub unsafe fn new(heap_start_addr: usize, heap_size: usize) -> Self {
        assert_eq!(
            heap_start_addr % PAGE_SIZE,
            0,
            "Start address should be page aligned"
        );
        assert!(
            heap_size >= Self::MIN_SIZE,
            "Heap size should be greater or equal to minimum heap size"
        );
        assert_eq!(
            heap_size % Self::MIN_SIZE,
            0,
            "Heap size should be a multiple of minimum heap size"
        );
        let slab_size = heap_size / Self::NUM_OF_SLABS;

        Self {
            slab_64_bytes: Slab::new(heap_start_addr, slab_size),
            slab_128_bytes: Slab::new(heap_start_addr + slab_size, slab_size),
            slab_256_bytes: Slab::new(heap_start_addr + 2 * slab_size, slab_size),
            slab_512_bytes: Slab::new(heap_start_addr + 3 * slab_size, slab_size),
            slab_1024_bytes: Slab::new(heap_start_addr + 4 * slab_size, slab_size),
            slab_2048_bytes: Slab::new(heap_start_addr + 5 * slab_size, slab_size),
            slab_4096_bytes: Slab::new(heap_start_addr + 6 * slab_size, slab_size),
            linked_list: linked_list_allocator::Heap::new(
                (heap_start_addr + 7 * slab_size) as *mut u8,
                slab_size,
            ),
        }
    }

    pub fn allocate(&mut self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let alloc_kind = Self::layout_to_alloc_kind(layout);

        let ptr = match alloc_kind {
            AllocatorKind::LinkedListAllocator => self
                .linked_list
                .allocate_first_fit(layout)
                .map_err(|_| AllocError)?,
            AllocatorKind::Slab64 => self.slab_64_bytes.allocate()?,
            AllocatorKind::Slab128 => self.slab_128_bytes.allocate()?,
            AllocatorKind::Slab256 => self.slab_256_bytes.allocate()?,
            AllocatorKind::Slab512 => self.slab_512_bytes.allocate()?,
            AllocatorKind::Slab1024 => self.slab_1024_bytes.allocate()?,
            AllocatorKind::Slab2048 => self.slab_2048_bytes.allocate()?,
            AllocatorKind::Slab4096 => self.slab_4096_bytes.allocate()?,
        };

        Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
    }

    pub fn deallocate(&mut self, ptr: NonNull<u8>, layout: Layout) {
        let alloc_kind = Self::layout_to_alloc_kind(layout);

        match alloc_kind {
            AllocatorKind::LinkedListAllocator => unsafe {
                self.linked_list.deallocate(ptr, layout)
            },
            AllocatorKind::Slab64 => self.slab_64_bytes.deallocate(ptr),
            AllocatorKind::Slab128 => self.slab_128_bytes.deallocate(ptr),
            AllocatorKind::Slab256 => self.slab_256_bytes.deallocate(ptr),
            AllocatorKind::Slab512 => self.slab_512_bytes.deallocate(ptr),
            AllocatorKind::Slab1024 => self.slab_1024_bytes.deallocate(ptr),
            AllocatorKind::Slab2048 => self.slab_2048_bytes.deallocate(ptr),
            AllocatorKind::Slab4096 => self.slab_4096_bytes.deallocate(ptr),
        }
    }

    fn layout_to_alloc_kind(layout: Layout) -> AllocatorKind {
        if layout.size() > 4096 {
            AllocatorKind::LinkedListAllocator
        } else if layout.size() <= 64 && layout.align() <= 64 {
            AllocatorKind::Slab64
        } else if layout.size() <= 128 && layout.align() <= 128 {
            AllocatorKind::Slab128
        } else if layout.size() <= 256 && layout.align() <= 256 {
            AllocatorKind::Slab256
        } else if layout.size() <= 512 && layout.align() <= 512 {
            AllocatorKind::Slab512
        } else if layout.size() <= 1024 && layout.align() <= 1024 {
            AllocatorKind::Slab1024
        } else if layout.size() <= 2048 && layout.align() <= 2048 {
            AllocatorKind::Slab2048
        } else {
            AllocatorKind::Slab4096
        }
    }
}
