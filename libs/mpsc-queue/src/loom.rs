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
        pub(crate) use loom::thread;

        pub(crate) mod alloc {
            #![allow(dead_code)]
            use core::fmt;
            use loom::alloc;
            /// Track allocations, detecting leaks
            ///
            /// This is a version of `loom::alloc::Track` that adds a missing
            /// `Default` impl.
            pub struct Track<T>(alloc::Track<T>);

            impl<T> Track<T> {
                /// Track a value for leaks
                #[inline(always)]
                pub fn new(value: T) -> Track<T> {
                    Track(alloc::Track::new(value))
                }

                /// Get a reference to the value
                #[inline(always)]
                pub fn get_ref(&self) -> &T {
                    self.0.get_ref()
                }

                /// Get a mutable reference to the value
                #[inline(always)]
                pub fn get_mut(&mut self) -> &mut T {
                    self.0.get_mut()
                }

                /// Stop tracking the value for leaks
                #[inline(always)]
                pub fn into_inner(self) -> T {
                    self.0.into_inner()
                }
            }

            impl<T: fmt::Debug> fmt::Debug for Track<T> {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    self.0.fmt(f)
                }
            }

            impl<T: Default> Default for Track<T> {
                fn default() -> Self {
                    Self::new(T::default())
                }
            }
        }
    } else {
        #[cfg(test)]
        #[inline(always)]
        pub(crate) fn model<R>(f: impl FnOnce() -> R) -> R {
            f()
        }

        pub(crate) mod sync {
            pub use core::sync::*;
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

        pub(crate) mod alloc {
            #[cfg(test)]
            use std::sync::Arc;

            #[cfg(test)]
            pub(in crate::loom) mod track {
                use std::{
                    cell::RefCell,
                    sync::{
                        atomic::{AtomicBool, Ordering},
                        Arc, Mutex, Weak,
                    },
                };

                #[derive(Clone, Debug, Default)]
                pub(crate) struct Registry(Arc<Mutex<RegistryInner>>);

                #[derive(Debug, Default)]
                struct RegistryInner {
                    tracks: Vec<Weak<TrackData>>,
                    next_id: usize,
                }

                #[derive(Debug)]
                pub(super) struct TrackData {
                    was_leaked: AtomicBool,
                    type_name: &'static str,
                    location: &'static core::panic::Location<'static>,
                    id: usize,
                }

                thread_local! {
                    static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) };
                }

                impl Registry {
                    pub(in crate::loom) fn current() -> Option<Registry> {
                        REGISTRY.with(|current| current.borrow().clone())
                    }

                    pub(in crate::loom) fn set_default(&self) -> impl Drop {
                        struct Unset(Option<Registry>);
                        impl Drop for Unset {
                            fn drop(&mut self) {
                                let _ =
                                    REGISTRY.try_with(|current| *current.borrow_mut() = self.0.take());
                            }
                        }

                        REGISTRY.with(|current| {
                            let mut current = current.borrow_mut();
                            let unset = Unset(current.clone());
                            *current = Some(self.clone());
                            unset
                        })
                    }

                    #[track_caller]
                    pub(super) fn start_tracking<T>() -> Option<Arc<TrackData>> {
                        // we don't use `Option::map` here because it creates a
                        // closure, which breaks `#[track_caller]`, since the caller
                        // of `insert` becomes the closure, which cannot have a
                        // `#[track_caller]` attribute on it.
                        #[allow(clippy::manual_map)]
                        match Self::current() {
                            Some(registry) => Some(registry.insert::<T>()),
                            _ => None,
                        }
                    }

                    #[track_caller]
                    pub(super) fn insert<T>(&self) -> Arc<TrackData> {
                        let mut inner = self.0.lock().unwrap();
                        let id = inner.next_id;
                        inner.next_id += 1;
                        let location = core::panic::Location::caller();
                        let type_name = std::any::type_name::<T>();
                        let data = Arc::new(TrackData {
                            type_name,
                            location,
                            id,
                            was_leaked: AtomicBool::new(false),
                        });
                        let weak = Arc::downgrade(&data);
                        tracing::trace!(
                            target: "mpsc-queue::alloc",
                            id,
                            "type" = %type_name,
                            %location,
                            "started tracking allocation",
                        );
                        inner.tracks.push(weak);
                        data
                    }

                    pub(in crate::loom) fn check(&self) {
                        let leaked = self
                            .0
                            .lock()
                            .unwrap()
                            .tracks
                            .iter()
                            .filter_map(|weak| {
                                let data = weak.upgrade()?;
                                data.was_leaked.store(true, Ordering::SeqCst);
                                Some(format!(
                                    " - id {}, {} allocated at {}",
                                    data.id, data.type_name, data.location
                                ))
                            })
                            .collect::<Vec<_>>();
                        if !leaked.is_empty() {
                            let leaked = leaked.join("\n  ");
                            panic!("the following allocations were leaked:\n  {leaked}");
                        }
                    }
                }

                impl Drop for TrackData {
                    fn drop(&mut self) {
                        if !self.was_leaked.load(Ordering::SeqCst) {
                            tracing::trace!(
                                target: "mpsc-queue::alloc",
                                id = self.id,
                                "type" = %self.type_name,
                                location = %self.location,
                                "dropped all references to a tracked allocation",
                            );
                        }
                    }
                }
            }

            /// Track allocations, detecting leaks
            #[derive(Debug, Default)]
            #[cfg(test)]
            pub struct Track<T> {
                value: T,

                track: Option<Arc<track::TrackData>>,
            }

            #[cfg(test)]
            impl<T> Track<T> {
                pub const fn new_const(value: T) -> Track<T> {
                    Track {
                        value,

                        track: None,
                    }
                }

                /// Track a value for leaks
                #[inline(always)]
                #[track_caller]
                pub fn new(value: T) -> Track<T> {
                    Track {
                        value,

                        #[cfg(test)]
                        track: track::Registry::start_tracking::<T>(),
                    }
                }

                /// Get a reference to the value
                #[inline(always)]
                pub fn get_ref(&self) -> &T {
                    &self.value
                }

                /// Get a mutable reference to the value
                #[inline(always)]
                pub fn get_mut(&mut self) -> &mut T {
                    &mut self.value
                }

                /// Stop tracking the value for leaks
                #[inline(always)]
                pub fn into_inner(self) -> T {
                    self.value
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
