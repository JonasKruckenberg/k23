// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Shim routing the concurrency primitives to `loom` under `--cfg=loom`, to `std` under
//! `cfg(test)`, and to `core` otherwise. Lets one test body compile under all three configs.

cfg_if::cfg_if! {
    if #[cfg(loom)] {
        pub(crate) use loom::model;
        pub(crate) use loom::sync;
        pub(crate) use loom::thread;
    } else {
        #[cfg(not(test))]
        pub(crate) use core::sync;
        #[cfg(test)]
        pub(crate) use std::sync;
        #[cfg(test)]
        pub(crate) use std::thread;

        /// Runs the model once. Under `cfg(loom)` this is loom's exhaustive checker.
        #[cfg(test)]
        #[inline(always)]
        pub(crate) fn model<F>(f: F)
        where
            F: Fn() + Sync + Send + 'static,
        {
            f();
        }
    }
}
