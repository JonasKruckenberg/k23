// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

cfg_if::cfg_if! {
    if #[cfg(loom)] {
        pub(crate) use loom::{cell, lazy_static, sync, thread_local};
    } else {
        pub(crate) use core::sync;

        pub(crate) mod cell {
            #[repr(transparent)]
            pub(crate) struct UnsafeCell<T>(core::cell::UnsafeCell<T>);

            impl<T> UnsafeCell<T> {
                pub(crate) const fn new(data: T) -> UnsafeCell<T> {
                    UnsafeCell(core::cell::UnsafeCell::new(data))
                }

                #[inline(always)]
                pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
                    f(self.0.get())
                }

                #[inline(always)]
                pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
                    f(self.0.get())
                }
            }
        }
    }
}
