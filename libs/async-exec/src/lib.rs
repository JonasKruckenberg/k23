#![feature(allocator_api)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(loom, feature(arbitrary_self_types))]
#![feature(const_type_id)]

extern crate alloc;

pub mod executor;
mod loom;
pub mod park;
pub mod scheduler;
pub mod sync;
pub mod task;
pub mod time;
