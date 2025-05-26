#![feature(allocator_api)]
#![cfg_attr(not(test), no_std)]
#![cfg_attr(loom, feature(arbitrary_self_types))]
#![feature(const_type_id)]
#![feature(thread_local)]
#![feature(debug_closure_helpers)]
extern crate alloc;

pub mod executor;
mod loom;
pub mod park;
pub mod scheduler;
pub mod sync;
pub mod task;
#[cfg(test)]
mod test_util;
pub mod time;
