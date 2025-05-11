// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Shared utility types & function for k23

#![cfg_attr(not(test), no_std)]

mod cache_padded;
mod checked_maybe_uninit;
mod loom;

pub use cache_padded::CachePadded;
pub use checked_maybe_uninit::{CheckedMaybeUninit, MaybeUninitExt};
