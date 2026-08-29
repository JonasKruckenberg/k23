// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Shim routing the atomics to `loom` under `--cfg=loom`, to `std` under `cfg(test)`, and to
//! `core` otherwise. Lets one test body compile under all three configs.
//!
//! The loom models need loom ≥ the fix for [tokio-rs/loom#416]: `block_on` built its waker vtable
//! from a promoted temporary, which optimized builds duplicate per codegen unit, so a cloned waker
//! failed [`core::task::Waker::will_wake`] against the waker it came from. Every primitive that
//! caches a waker — `WaitCell`, `WaitQueue` — then replaced *and woke* its stored waker on each
//! poll, so a model self-woke forever and died with "exceeded maximum number of branches".
//!
//! [tokio-rs/loom#416]: https://github.com/tokio-rs/loom/issues/416

cfg_if::cfg_if! {
    if #[cfg(loom)] {
        pub(crate) use loom::sync;
        #[cfg(test)]
        pub(crate) use loom::{future, model, thread};
        #[cfg(test)]
        pub(crate) use loom::lazy_static;
    } else {
        #[cfg(not(test))]
        pub(crate) use core::sync;
        #[cfg(test)]
        pub(crate) use std::sync;
        #[cfg(test)]
        pub(crate) use std::thread;
    }
}
