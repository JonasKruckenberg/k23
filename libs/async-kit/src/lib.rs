// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(all(not(test), target_os = "none"), no_std)]
#![feature(allocator_api)]
#![feature(const_type_id)]
#![feature(debug_closure_helpers)]
#![cfg_attr(loom, feature(arbitrary_self_types))]

extern crate alloc;

mod loom;
pub mod park;
pub mod scheduler;
pub mod sync;
pub mod task;
pub mod time;
mod executor;
