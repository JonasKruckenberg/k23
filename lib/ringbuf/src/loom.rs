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
        pub(crate) use loom::cell;
        pub(crate) use loom::model;
        pub(crate) use loom::sync;
        pub(crate) use loom::thread;

        /// Yield to the model checker. A bare spin loop would let loom explore an
        /// interleaving where the spinning thread never observes the other's store.
        #[inline]
        pub(crate) fn spin_hint() {
            loom::thread::yield_now();
        }
    } else {
        #[cfg(not(test))]
        pub(crate) use core::sync;
        #[cfg(test)]
        pub(crate) use std::sync;
        #[cfg(test)]
        pub(crate) use std::thread;

        #[inline(always)]
        pub(crate) fn spin_hint() {
            core::hint::spin_loop();
        }

        /// Runs the model once. Under `cfg(loom)` this is loom's exhaustive checker.
        #[cfg(test)]
        #[inline(always)]
        pub(crate) fn model<F>(f: F)
        where
            F: Fn() + Sync + Send + 'static,
        {
            f();
        }

        pub(crate) mod cell {
            /// Mirrors `loom::cell::UnsafeCell`'s `with`/`with_mut` API over the real one so
            /// call sites are config-independent.
            #[derive(Debug)]
            #[repr(transparent)]
            pub(crate) struct UnsafeCell<T: ?Sized>(core::cell::UnsafeCell<T>);

            impl<T> UnsafeCell<T> {
                pub(crate) const fn new(data: T) -> UnsafeCell<T> {
                    UnsafeCell(core::cell::UnsafeCell::new(data))
                }
            }

            impl<T: ?Sized> UnsafeCell<T> {
                #[inline(always)]
                pub(crate) fn with<F, R>(&self, f: F) -> R
                where
                    F: FnOnce(*const T) -> R,
                {
                    f(self.0.get())
                }

                #[inline(always)]
                pub(crate) fn with_mut<F, R>(&self, f: F) -> R
                where
                    F: FnOnce(*mut T) -> R,
                {
                    f(self.0.get())
                }
            }
        }
    }
}
