use crate::allocator::locked::LockedHeap;

mod heap;
mod locked;
mod slab;

#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {}
