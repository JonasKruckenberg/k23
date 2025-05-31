//! Async executor and supporting infrastructure for k23 cooperative multitasking.
//!
//! This crate was heavily inspired by tokio and the (much better) maitake crates, to a small extend smol also influenced the design.  


#![feature(allocator_api)]
#![cfg_attr(not(test), no_std)]
#![cfg_attr(loom, feature(arbitrary_self_types))]
#![feature(const_type_id)]
#![feature(thread_local)]
#![feature(debug_closure_helpers)]
#![feature(context_ext)]
#![feature(local_waker)]
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
