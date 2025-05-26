// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(loom)] {
        pub(crate) use loom::sync;
        pub(crate) use loom::cell;
        pub(crate) use loom::model;
        #[cfg(test)]
        pub(crate) use loom::thread;
        pub(crate) use loom::lazy_static;
    } else {
        #[cfg(test)]
        pub(crate) use std::thread;
        #[cfg(test)]
        pub(crate) use lazy_static::lazy_static;

        #[cfg(test)]
        #[inline(always)]
        pub(crate) fn model<R>(f: impl FnOnce() -> R) -> R {
            f()
        }

        pub(crate) mod sync {
            pub use core::sync::*;
            pub use alloc::sync::*;
            #[cfg(test)]
            pub(crate) use std::sync::*;
        }

        pub(crate) mod cell {
            #[derive(Debug)]
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

//             #[derive(Debug)]
//             #[pin_project::pin_project]
//             pub(crate) struct TrackFuture<F> {
//                 #[pin]
//                 inner: F,
//                 track: Arc<()>,
//             }
//
//             impl<F: Future> Future for TrackFuture<F> {
//                 type Output = TrackFuture<F::Output>;
//                 fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//                     let this = self.project();
//                     this.inner.poll(cx).map(|inner| TrackFuture {
//                         inner,
//                         track: this.track.clone(),
//                     })
//                 }
//             }
//
//             impl<F> TrackFuture<F> {
//                 /// Wrap a `Future` in a `TrackFuture` that participates in Loom's
//                 /// leak checking.
//                 #[track_caller]
//                 pub(crate) fn new(inner: F) -> Self {
//                     Self {
//                         inner,
//                         track: Arc::new(()),
//                     }
//                 }
//
//                 /// Stop tracking this future, and return the inner value.
//                 pub(crate) fn into_inner(self) -> F {
//                     self.inner
//                 }
//             }
//
//             #[track_caller]
//             pub(crate) fn track_future<F: Future>(inner: F) -> TrackFuture<F> {
//                 TrackFuture::new(inner)
//             }
//
//             // PartialEq impl so that `assert_eq!(..., Ok(...))` works
//             impl<F: PartialEq> PartialEq for TrackFuture<F> {
//                 fn eq(&self, other: &Self) -> bool {
//                     self.inner == other.inner
//                 }
//             }
//         }
//     } else {

//
//
//     }
// }
