// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use crate::park::Park;
use core::fmt;
use core::task::{RawWaker, RawWakerVTable, Waker};
use static_assertions::assert_impl_all;

const STATE_EMPTY: usize = 0;
const STATE_PARKED: usize = 1;
const STATE_NOTIFIED: usize = 2;

pub struct Parker<P>(Arc<Inner<P>>);

#[derive(Clone)]
pub struct UnparkToken<P>(Parker<P>);
assert_impl_all!(UnparkToken<()>: Send, Sync);

#[derive(Debug)]
struct Inner<P> {
    state: AtomicUsize,
    park_impl: P,
}

#[derive(Debug)]
pub enum UnparkError {
    /// The target thread/cpu wasn't parked (yet). This, likely means the caller attempted to unpark
    /// the target too early and should wait a bit.
    NotParked,
    /// The target thread/cpu was already unparked.
    AlreadyUnparked,
}

// === impl Parker ===

impl<P> fmt::Debug for Parker<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Parker")
            .field("state", &self.0.describe_state())
            .finish_non_exhaustive()
    }
}

impl<P> Clone for Parker<P> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<P: Park> Parker<P> {
    pub fn new(park_impl: P) -> Self {
        Self(Arc::new(Inner {
            state: AtomicUsize::new(STATE_EMPTY),
            park_impl,
        }))
    }

    #[inline]
    pub fn park(&self) {
        self.0.park();
    }

    /// Attempts to unpark itself, returning an error if that fails.
    ///
    /// This method isn't terribly useful, but in certain circumstances (e.g. in an interrupt handler)
    /// may allow the CPU to wake itself up correctly.
    ///
    /// # Errors
    ///
    /// The returned [`UnparkError`] describes why unparking failed.
    #[inline]
    pub fn try_unpark(&self) -> Result<(), UnparkError> {
        self.0.try_unpark()
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
        Inner::into_waker(self.0)
    }
}

// === impl UnparkToken ===

impl<P> fmt::Debug for UnparkToken<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnparkToken")
            .field("parker", &self.0)
            .finish()
    }
}

impl<P: Park> UnparkToken<P> {
    /// Attempts to unpark the target, returning an error if the target is not parked.
    ///
    /// # Errors
    ///
    /// The returned [`UnparkError`] describes why unparking failed.
    #[inline]
    pub fn try_unpark(&self) -> Result<(), UnparkError> {
        self.0.try_unpark()
    }

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

// === impl Inner ===

impl<P> Inner<P> {
    fn describe_state(&self) -> &'static str {
        match self.state.load(Ordering::Acquire) {
            STATE_EMPTY => "<empty>",
            STATE_PARKED => "<parked>",
            STATE_NOTIFIED => "<notified>",
            _ => "<unknown>",
        }
    }
}

impl<P: Park> Inner<P> {
    fn park(&self) {
        tracing::trace!(
            state = self.describe_state(),
            "parking execution context..."
        );

        // If we were previously notified then we consume this notification and
        // return quickly.
        if self
            .state
            .compare_exchange(
                STATE_NOTIFIED,
                STATE_EMPTY,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            return;
        }

        match self.state.compare_exchange(
            STATE_EMPTY,
            STATE_PARKED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => {}
            Err(STATE_NOTIFIED) => {
                // We must read here, even though we know it will be `NOTIFIED`.
                // This is because `unpark` may have been called again since we read
                // `NOTIFIED` in the `compare_exchange` above. We must perform an
                // acquire operation that synchronizes with that `unpark` to observe
                // any writes it made before the call to unpark. To do that we must
                // read from the write it made to `state`.
                let old = self.state.swap(STATE_EMPTY, Ordering::SeqCst);
                debug_assert_eq!(old, STATE_NOTIFIED, "park state changed unexpectedly");

                return;
            }
            Err(actual) => panic!("inconsistent park state; actual = {actual}"),
        }

        loop {
            self.park_impl.park();

            if self
                .state
                .compare_exchange(
                    STATE_NOTIFIED,
                    STATE_EMPTY,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                // we got unparked
                return;
            }

            tracing::trace!("spurious wakeup, going back to sleep...");
        }
    }

    /// Attempts to unpark the target, returning an error if the target is not parked.
    ///
    /// # Errors
    ///
    /// The returned [`UnparkError`] describes why unparking failed.
    fn try_unpark(&self) -> Result<(), UnparkError> {
        match self.state.swap(STATE_NOTIFIED, Ordering::SeqCst) {
            STATE_EMPTY => return Err(UnparkError::NotParked),
            STATE_NOTIFIED => return Err(UnparkError::AlreadyUnparked),
            STATE_PARKED => {}
            _ => panic!("inconsistent state in unpark"),
        }

        self.park_impl.unpark();

        Ok(())
    }

    /// Unparks the target, panicking if the target is not actually parked.
    ///
    /// # Panics
    ///
    /// Panics if the target is not parked.
    fn unpark(&self) {
        self.try_unpark().expect("already unparked");
    }

    fn into_raw(this: Arc<Self>) -> *const () {
        Arc::into_raw(this).cast::<()>()
    }

    unsafe fn from_raw(ptr: *const ()) -> Arc<Self> {
        // Safety: ensured by caller
        unsafe { Arc::from_raw(ptr.cast::<Self>()) }
    }

    // === Waker functionality ===

    unsafe fn waker_clone(raw: *const ()) -> RawWaker {
        // Safety: ensured by VTable
        unsafe {
            Arc::increment_strong_count(raw.cast::<Self>());
            Self::into_raw_waker(Inner::from_raw(raw))
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
        let unparker = unsafe { Self::from_raw(raw) };
        let _ = unparker.try_unpark();
    }

    unsafe fn waker_wake_by_ref(raw: *const ()) {
        let raw = raw.cast::<Self>();
        // Safety: ensured by VTable
        unsafe {
            let _ = (*raw).try_unpark();
        }
    }

    fn into_raw_waker(this: Arc<Self>) -> RawWaker {
        RawWaker::new(
            Inner::into_raw(this),
            &RawWakerVTable::new(
                Self::waker_clone,
                Self::waker_wake,
                Self::waker_wake_by_ref,
                Self::waker_drop_waker,
            ),
        )
    }

    fn into_waker(this: Arc<Self>) -> Waker {
        // Safety: the vtable functions are fine, see above
        unsafe {
            let raw = Self::into_raw_waker(this);
            Waker::from_raw(raw)
        }
    }
}

// === impl UnparkError ===

impl fmt::Display for UnparkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnparkError::NotParked => f.write_str("not parked"),
            UnparkError::AlreadyUnparked => f.write_str("already unparked"),
        }
    }
}

impl core::error::Error for UnparkError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    use crate::loom::thread;
    use crate::park::StdPark;
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
