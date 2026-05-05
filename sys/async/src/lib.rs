// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Async executor and supporting infrastructure for k23 cooperative multitasking.
//!
//! This crate was heavily inspired by tokio and the (much better) maitake crates, to a small extend smol also influenced the design.

#![cfg_attr(not(any(test, feature = "__bench")), no_std)]
#![feature(debug_closure_helpers)]
#![feature(never_type)]
#![feature(allocator_api)]
extern crate alloc;

mod error;
pub mod executor;
pub mod loom;
pub mod sync;
pub mod task;
pub mod time;

pub use error::{Closed, SpawnError};
pub use futures::future;

cfg_if::cfg_if! {
    if #[cfg(feature = "__bench")]  {
        pub mod test_util;
    } else
    if #[cfg(test)] {
        mod test_util;
    }
}
