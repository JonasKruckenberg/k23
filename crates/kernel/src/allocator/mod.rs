use crate::allocator::locked::LockedHeap;
use crate::board_info::BoardInfo;

mod heap;
mod locked;
mod slab;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init(board_info: &BoardInfo) {
    // extern "C" {
    //     static __stack_start: u8;
    // }

    // let stack_area_base = unsafe { addr_of!(__stack_start) };
    //
    // let heap_base = unsafe { stack_area_base.add(arch::STACK_SIZE_PAGES * arch::PAGE_SIZE * board_info.cpus) };
    // let heap_base_aligned = (heap_base as usize + (PAGE_SIZE - 1)) & !(PAGE_SIZE - 1);
    //
    // let heap_size = (board_info.memory.end - heap_base_aligned) & !(Heap::MIN_SIZE - 1);

    // unsafe {
    //     ALLOCATOR.init(heap_base_aligned, heap_size);
    // }
}
