mod aligned_vec;
mod code_memory;
mod guest_allocator;

pub use aligned_vec::AlignedVec;
use alloc::vec::Vec;
pub use code_memory::CodeMemory;
pub use guest_allocator::GuestAllocator;

pub type GuestVec<T> = Vec<T, GuestAllocator>;
