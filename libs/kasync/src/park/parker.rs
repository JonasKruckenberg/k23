// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::Arc;
use crate::park::Park;
use crate::time::{Clock, Deadline};
use core::task::{RawWaker, RawWakerVTable, Waker};
use static_assertions::assert_impl_all;

#[derive(Debug)]
pub struct Parker<P>(Arc<P>);

#[derive(Debug, Clone)]
pub struct UnparkToken<P>(Parker<P>);
assert_impl_all!(UnparkToken<()>: Send, Sync);

// === impl Parker ===

impl<P> Clone for Parker<P> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<P: Park> Parker<P> {
    pub fn new(park_impl: P) -> Self {
        Self(Arc::new(park_impl))
    }

    #[inline]
    pub fn park(&self) {
        self.0.park();
    }

    #[inline]
    pub fn park_until(&self, deadline: Deadline, clock: &Clock) {
        self.0.park_until(deadline, clock);
    }

    /// Attempts to unpark itself, panicking if that fails.
    ///
    /// This method isn't terribly useful, but in certain circumstances (e.g. in an interrupt handler)
    /// may allow the CPU to wake itself up correctly.
    ///
    /// # Panics
    ///
    /// Panics if the target is not parked.
    #[inline]
    pub fn unpark(&self) {
        self.0.unpark();
    }

    /// Convert this [`Parker`] into an [`UnparkToken`] which can be used to wake up this thread/core.
    #[inline]
    pub fn into_unpark(self) -> UnparkToken<P> {
        UnparkToken(self)
    }

    /// Convert self into an async Rust compatible `Waker` which will wake this thread/core through
    /// its waking method.
    #[inline]
    pub fn into_waker(self) -> Waker {
        // Safety: the vtable functions are fine, see above
        unsafe {
            let raw = Self::into_raw_waker(self.0);
            Waker::from_raw(raw)
        }
    }

    fn into_raw(this: Arc<P>) -> *const () {
        Arc::into_raw(this).cast::<()>()
    }

    unsafe fn from_raw(ptr: *const ()) -> Arc<P> {
        // Safety: ensured by caller
        unsafe { Arc::from_raw(ptr.cast::<P>()) }
    }

    const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::waker_clone,
        Self::waker_wake,
        Self::waker_wake_by_ref,
        Self::waker_drop_waker,
    );

    unsafe fn waker_clone(raw: *const ()) -> RawWaker {
        // Safety: ensured by VTable
        unsafe {
            Arc::increment_strong_count(raw.cast::<Self>());
            Self::into_raw_waker(Self::from_raw(raw))
        }
    }

    unsafe fn waker_drop_waker(raw: *const ()) {
        // Safety: ensured by VTable
        unsafe {
            drop(Self::from_raw(raw));
        }
    }

    unsafe fn waker_wake(raw: *const ()) {
        // Safety: ensured by VTable
        let park = unsafe { Self::from_raw(raw) };
        park.unpark();
    }

    unsafe fn waker_wake_by_ref(raw: *const ()) {
        let park = raw.cast::<Self>();
        // Safety: ensured by VTable
        unsafe {
            (*park).unpark();
        }
    }

    // `Waker::will_wake` is used all over the place to optimize waker code (e.g. only update wakers if they
    // have a different wake target). Problem is `will_wake` only checks for pointer equality and since
    // the `into_raw_waker` would usually be inlined in release mode (and with it `WAKER_VTABLE`) the
    // Waker identity would be different before and after calling `.clone()`. This isn't a correctness
    // problem since it's still the same waker in the end, it just causes a lot of unnecessary wake ups.
    // the `inline(never)` below is therefore quite load-bearing
    #[inline(never)]
    fn into_raw_waker(this: Arc<P>) -> RawWaker {
        RawWaker::new(Self::into_raw(this), &Self::WAKER_VTABLE)
    }
}

// === impl UnparkToken ===

impl<P: Park> UnparkToken<P> {
    /// Unparks the target, panicking if the target is not actually parked.
    ///
    /// # Panics
    ///
    /// Panics if the target is not parked.
    #[inline]
    pub fn unpark(&self) {
        self.0.0.unpark();
    }

    /// Convert self into an async Rust compatible `Waker` which will wake the target thread/core
    /// through its waking method.
    #[inline]
    pub fn into_waker(self) -> Waker {
        self.0.into_waker()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StdPark;
    use crate::loom::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    use crate::loom::thread;
    use core::pin::{Pin, pin};
    use core::task::{Context, Poll, Waker};

    #[test]
    fn parking_basically_works() {
        // What is going on in this test? Well basically we want to assert that a thread (thread A)
        // can park itself using `Parker::park` and that another thread (thread B) can wake it back up.
        // To do this, we construct the `Parker` on thread A, create an `UnparkToken` for it, which
        // we then send to thread B through a channel.
        // Having sent the token, we park ourselves waiting to be unparked.

        crate::loom::model(|| {
            crate::loom::lazy_static! {
                static ref A_UNPARKED: AtomicBool = AtomicBool::new(false);
            }
            let (tx, rx) = mpsc::channel();

            // Thread A will suspend itself
            let a = thread::spawn(move || {
                let parker = Parker::new(StdPark::for_current());

                // send over the UnparkToken
                tx.send(parker.clone().into_unpark()).unwrap();

                // and finally park ourselves!
                parker.park();

                A_UNPARKED.store(true, Ordering::Release);
            });

            // Thread B will just wake up thread A
            let b = thread::spawn(move || {
                // obtain the token sent through the channel
                let unpark = rx.recv().unwrap();

                // and unpark thread A
                unpark.unpark();
            });

            let _ = a.join();
            let _ = b.join();

            assert!(A_UNPARKED.load(Ordering::Acquire));
        });
    }

    #[test]
    fn waker() {
        // This is almost the same test as above, but through the Waker indirection (and a simulated
        // future poll loop).

        crate::loom::model(|| {
            crate::loom::lazy_static! {
                static ref NUM_POLLS: AtomicUsize = AtomicUsize::new(0);
                static ref COMPLETED: AtomicBool = AtomicBool::new(false);
            }

            let (tx, rx) = mpsc::channel();

            // Thread A will suspend itself
            let a = thread::spawn(move || {
                struct Yield {
                    done: bool,
                    tx: crate::loom::sync::mpsc::Sender<Waker>,
                }
                impl Future for Yield {
                    type Output = ();

                    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                        if !self.done {
                            self.done = true;
                            self.tx.send(cx.waker().clone()).unwrap();
                            Poll::Pending
                        } else {
                            Poll::Ready(())
                        }
                    }
                }

                let parker = Parker::new(StdPark::for_current());
                let waker = parker.clone().into_waker();

                let mut cx = Context::from_waker(&waker);
                let mut future = pin!(Yield { done: false, tx });

                loop {
                    NUM_POLLS.fetch_add(1, Ordering::Release);
                    if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                        COMPLETED.store(true, Ordering::Release);
                        return v;
                    }

                    parker.park();
                }
            });

            // Thread B will just wake up thread A
            let b = thread::spawn(move || {
                // obtain the token sent through the channel
                let waker = rx.recv().unwrap();

                // and unpark thread A
                waker.wake();
            });

            let _ = a.join();
            let _ = b.join();

            assert!(COMPLETED.load(Ordering::Acquire));
            assert_eq!(NUM_POLLS.load(Ordering::Acquire), 2);
        });
    }
}
