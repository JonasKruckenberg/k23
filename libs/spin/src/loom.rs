// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

cfg_if::cfg_if! {
    if #[cfg(loom)] {
        pub(crate) use loom::sync;
        pub(crate) use loom::cell;
        pub(crate) use loom::thread;
    } else {
        #[cfg(test)]
        pub(crate) use std::thread;

        #[cfg(test)]
        #[inline(always)]
        pub(crate) fn model<F>(f: F)
        where
            F: Fn() + Sync + Send + 'static,
        {
            f()
        }

        pub(crate) mod sync {
            pub(crate) use core::sync::*;


            #[cfg(test)]
            pub(crate) use std::sync::*;
        }

        pub(crate) mod cell {
            #[derive(Debug)]
            #[repr(transparent)]
            pub(crate) struct UnsafeCell<T: ?Sized>(core::cell::UnsafeCell<T>);

            impl<T> UnsafeCell<T> {
                pub const fn new(data: T) -> UnsafeCell<T> {
                    UnsafeCell(core::cell::UnsafeCell::new(data))
                }
            }

            impl<T: ?Sized> UnsafeCell<T> {
                #[inline(always)]
                pub fn with<F, R>(&self, f: F) -> R
                where
                    F: FnOnce(*const T) -> R,
                {
                    f(self.0.get())
                }
                #[inline(always)]
                pub fn with_mut<F, R>(&self, f: F) -> R
                where
                    F: FnOnce(*mut T) -> R,
                {
                    f(self.0.get())
                }
            }
            impl<T> UnsafeCell<T> {
                #[inline(always)]
                #[must_use]
                pub(crate) fn into_inner(self) -> T {
                    self.0.into_inner()
                }
            }
        }
    }
}
