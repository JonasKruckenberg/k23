use crate::allocator::locked::LockedHeap;
use crate::arch;
use crate::board_info::BoardInfo;

mod heap;
mod locked;
mod slab;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    unsafe {
        ALLOCATOR.init(arch::HEAP_BASE, arch::HEAP_SIZE);
    }
}
