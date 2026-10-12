// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

cfg_if::cfg_if! {
    if #[cfg(loom)] {
        pub(crate) use loom::cell;
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

        #[cfg(test)]
        #[inline(always)]
        pub(crate) fn model<F>(f: F)
        where
            F: Fn() + Sync + Send + 'static,
        {
            f();
        }

        pub(crate) mod cell {
            /// Under loom these guards keep the access registered for as long as they live, which
            /// is the whole point of holding one rather than a bare pointer. Here they are the
            /// bare pointer.
            #[derive(Debug)]
            #[repr(transparent)]
            pub struct MutPtr<T>(*mut T);
            #[derive(Debug)]
            #[repr(transparent)]
            pub struct ConstPtr<T>(*const T);

            /// Mirrors `loom::cell::UnsafeCell`'s guard API
            #[derive(Debug)]
            #[repr(transparent)]
            pub(crate) struct UnsafeCell<T: ?Sized>(core::cell::UnsafeCell<T>);

            impl<T> UnsafeCell<T> {
                #[inline(always)]
                pub(crate) const fn new(data: T) -> UnsafeCell<T> {
                    UnsafeCell(core::cell::UnsafeCell::new(data))
                }

                #[inline(always)]
                pub fn get(&self) -> ConstPtr<T> {
                    ConstPtr(self.0.get())
                }

                #[inline(always)]
                pub fn get_mut(&self) -> MutPtr<T> {
                    MutPtr(self.0.get())
                }
            }

            impl<T> ConstPtr<T> {
                #[inline]
                pub unsafe fn deref(&self) -> &T {
                    // Safety: ensured by caller
                    unsafe { &*self.0 }
                }

                #[inline]
                pub fn with<F, R>(&self, f: F) -> R
                where
                    F: FnOnce(*const T) -> R,
                {
                    f(self.0)
                }
            }

            impl<T> MutPtr<T> {
                #[inline]
                #[expect(clippy::mut_from_ref, reason = "mirrors loom::cell::MutPtr::deref")]
                pub unsafe fn deref(&self) -> &mut T {
                    // Safety: ensured by caller
                    unsafe { &mut *self.0 }
                }
            }
        }
    }
}
