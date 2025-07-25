// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::panic::{RefUnwindSafe, UnwindSafe};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use core::{fmt, task};

use bitflags::bitflags;
use static_assertions::const_assert_eq;
use util::{CachePadded, loom_const_fn};

use crate::error::Closed;
use crate::loom::cell::UnsafeCell;
use crate::loom::sync::atomic::{AtomicUsize, Ordering};

/// An atomically registered [`Waker`].
///
/// This cell stores the [`Waker`] of a single task. A [`Waker`] is stored in
/// the cell either by calling [`poll_wait`], or by polling a [`wait`]
/// future. Once a task's [`Waker`] is stored in a `WaitCell`, it can be woken
/// by calling [`wake`] on the `WaitCell`.
///
/// # Implementation Notes
///
/// This type is copied from [`maitake-sync`](https://github.com/hawkw/mycelium/blob/dd0020892564c77ee4c20ffbc2f7f5b046ad54c8/maitake-sync/src/wait_cell.rs)
/// which is in turn inspired by the [`AtomicWaker`] type used in Tokio's
/// synchronization primitives, with the following modifications:
///
/// - An additional bit of state is added to allow [setting a "close"
///   bit](Self::close).
/// - A `WaitCell` is always woken by value (for now).
///
/// [`AtomicWaker`]: https://github.com/tokio-rs/tokio/blob/09b770c5db31a1f35631600e1d239679354da2dd/tokio/src/sync/task/atomic_waker.rs
/// [`Waker`]: Waker
/// [`poll_wait`]: Self::poll_wait
/// [`wait`]: Self::wait
/// [`wake`]: Self::wake
pub struct WaitCell {
    state: CachePadded<AtomicUsize>,
    waker: UnsafeCell<Option<Waker>>,
}

bitflags! {
    #[derive(Debug, PartialEq, Eq)]
    struct State: usize {
        const WAITING = 0b0000;
        const REGISTERING = 0b0001;
        const WAKING = 0b0010;
        const WOKEN = 0b0100;
        const CLOSED = 0b1000;
    }
}
// WAITING MUST be zero
const_assert_eq!(State::WAITING.bits(), 0);

/// Future returned from [`WaitCell::wait()`].
///
/// This future is fused, so once it has completed, any future calls to poll
/// will immediately return [`Poll::Ready`].
#[derive(Debug)]
#[must_use = "futures do nothing unless `.await`ed or `poll`ed"]
pub struct Wait<'a> {
    /// The [`WaitCell`] being waited on.
    cell: &'a WaitCell,

    presubscribe: Poll<Result<(), Closed>>,
}

/// Future returned from [`WaitCell::subscribe()`].
///
/// See the documentation for [`WaitCell::subscribe()`] for details.
#[derive(Debug)]
#[must_use = "futures do nothing unless `.await`ed or `poll`ed"]
pub struct Subscribe<'a> {
    /// The [`WaitCell`] being waited on.
    cell: &'a WaitCell,
}

/// An error indicating that a [`WaitCell`] was closed or busy while
/// attempting register a [`Waker`].
///
/// This error is returned by the [`WaitCell::poll_wait`] method.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PollWaitError {
    /// The [`Waker`] was not registered because the [`WaitCell`] has been
    /// [closed](WaitCell::close).
    Closed,

    /// The [`Waker`] was not registered because another task was concurrently
    /// storing its own [`Waker`] in the [`WaitCell`].
    Busy,
}

// === impl WaitCell ===

impl WaitCell {
    loom_const_fn! {
        pub const fn new() -> Self {
            Self {
                state: CachePadded(AtomicUsize::new(State::WAITING.bits())),
                waker: UnsafeCell::new(None),
            }
        }
    }

    #[tracing::instrument]
    pub fn poll_wait(&self, cx: &mut Context<'_>) -> Poll<Result<(), PollWaitError>> {
        // this is based on tokio's AtomicWaker synchronization strategy
        match self.compare_exchange(State::WAITING, State::REGISTERING, Ordering::Acquire) {
            Err(actual) if actual.contains(State::CLOSED) => {
                return Poll::Ready(Err(PollWaitError::Closed));
            }
            Err(actual) if actual.contains(State::WOKEN) => {
                // take the wakeup
                self.fetch_and(!State::WOKEN, Ordering::Release);
                return Poll::Ready(Ok(()));
            }
            // someone else is notifying, so don't wait!
            Err(actual) if actual.contains(State::WAKING) => {
                return Poll::Ready(Ok(()));
            }
            Err(_) => return Poll::Ready(Err(PollWaitError::Busy)),
            Ok(_) => {}
        }

        let waker = cx.waker();
        tracing::trace!(
            /*wait_cell = ?fmt::ptr(self),*/ ?waker,
            "registering waker"
        );

        if let Some(prev_waker) = self.replace_waker(waker.clone()) {
            tracing::debug!("Replaced an old waker in cell, waking");
            prev_waker.wake();
        }

        if let Err(actual) =
            self.compare_exchange(State::REGISTERING, State::WAITING, Ordering::AcqRel)
        {
            // If the `compare_exchange` fails above, this means that we were notified for one of
            // two reasons: either the cell was awoken, or the cell was closed.
            //
            // Bail out of the parking state, and determine what to report to the caller.
            tracing::trace!(state = ?actual, "was notified");

            // Safety: No one else is touching the waker right now, so it is safe to access it
            // mutably
            let waker = self.waker.with_mut(|waker| unsafe { (*waker).take() });

            // Reset to the WAITING state by clearing everything *except*
            // the closed bits (which must remain set). This `fetch_and`
            // does *not* set the CLOSED bit if it is unset, it just doesn't
            // clear it.
            let state = self.fetch_and(State::CLOSED, Ordering::AcqRel);
            // The only valid state transition while we were parking is to
            // add the CLOSED bit.
            debug_assert!(
                state == actual || state == actual | State::CLOSED,
                "state changed unexpectedly while parking!"
            );

            if let Some(waker) = waker {
                waker.wake();
            }

            // Was the `CLOSED` bit set while we were clearing other bits?
            // If so, the cell is closed. Otherwise, we must have been notified.
            if state.contains(State::CLOSED) {
                return Poll::Ready(Err(PollWaitError::Closed));
            }

            return Poll::Ready(Ok(()));
        }

        // Waker registered, time to yield!
        Poll::Pending
    }

    /// Wait to be woken up by this cell.
    ///
    /// # Returns
    ///
    /// This future completes with the following values:
    ///
    /// - [`Ok`]`(())` if the future was woken by a call to [`wake`] or another
    ///   task calling [`poll_wait`] or [`wait`] on this [`WaitCell`].
    /// - [`Err`]`(`[`Closed`]`)` if the task was woken by a call to [`close`],
    ///   or the [`WaitCell`] was already closed.
    ///
    /// **Note**: The calling task's [`Waker`] is not registered until AFTER the
    /// first time the returned [`Wait`] future is polled. This means that if a
    /// call to [`wake`] occurs between when [`wait`] is called and when the
    /// future is first polled, the future will *not* complete. If the caller is
    /// responsible for performing an operation which will result in an eventual
    /// wakeup, prefer calling [`subscribe`] _before_ performing that operation
    /// and `.await`ing the [`Wait`] future returned by [`subscribe`].
    ///
    /// [`wake`]: Self::wake
    /// [`poll_wait`]: Self::poll_wait
    /// [`wait`]: Self::wait
    /// [`close`]: Self::close
    /// [`subscribe`]: Self::subscribe
    pub fn wait(&self) -> Wait<'_> {
        Wait {
            cell: self,
            presubscribe: Poll::Pending,
        }
    }

    /// Eagerly subscribe to notifications from this `WaitCell`.
    ///
    /// This method returns a [`Subscribe`] [`Future`], which outputs a [`Wait`]
    /// [`Future`]. Awaiting the [`Subscribe`] future will eagerly register the
    /// calling task to be woken by this [`WaitCell`], so that the returned
    /// [`Wait`] future will be woken by any calls to [`wake`] (or [`close`])
    /// that occur between when the [`Subscribe`] future completes and when the
    /// returned [`Wait`] future is `.await`ed.
    ///
    /// This is primarily intended for scenarios where the task that waits on a
    /// [`WaitCell`] is responsible for performing some operation that
    /// ultimately results in the [`WaitCell`] being woken. If the task were to
    /// simply perform the operation and then call [`wait`] on the [`WaitCell`],
    /// a potential race condition could occur where the operation completes and
    /// wakes the [`WaitCell`] *before* the [`Wait`] future is first `.await`ed.
    /// Using `subscribe`, the task can ensure that it is ready to be woken by
    /// the cell *before* performing an operation that could result in it being
    /// woken.
    ///
    /// These scenarios occur when a wakeup is triggered by another thread/CPU
    /// core in response to an operation performed in the task waiting on the
    /// `WaitCell`, or when the wakeup is triggered by a hardware interrupt
    /// resulting from operations performed in the task.
    ///
    /// [`wait`]: Self::wait
    /// [`wake`]: Self::wake
    /// [`close`]: Self::close
    pub fn subscribe(&self) -> Subscribe<'_> {
        Subscribe { cell: self }
    }

    /// Wake the [`Waker`] stored in this cell.
    ///
    /// # Returns
    ///
    /// - `true` if a waiting task was woken.
    /// - `false` if no task was woken (no [`Waker`] was stored in the cell)
    #[tracing::instrument]
    pub fn wake(&self) -> bool {
        if let Some(waker) = self.take_waker(false) {
            waker.wake();
            true
        } else {
            false
        }
    }

    /// Close the [`WaitCell`].
    ///
    /// This wakes any waiting task with an error indicating the `WaitCell` is
    /// closed. Subsequent calls to [`wait`] or [`poll_wait`] will return an
    /// error indicating that the cell has been closed.
    ///
    /// [`wait`]: Self::wait
    /// [`poll_wait`]: Self::poll_wait
    #[tracing::instrument]
    pub fn close(&self) -> bool {
        if let Some(waker) = self.take_waker(true) {
            waker.wake();
            true
        } else {
            false
        }
    }

    /// Returns `true` if this `WaitCell` is [closed](Self::close).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.current_state() == State::CLOSED
    }

    /// Asynchronously poll the given function `f` until a condition occurs,
    /// using the [`WaitCell`] to only re-poll when notified.
    ///
    /// This can be used to implement a "wait loop", turning a "try" function
    /// (e.g. "try_recv" or "try_send") into an asynchronous function (e.g.
    /// "recv" or "send").
    ///
    /// In particular, this function correctly *registers* interest in the [`WaitCell`]
    /// prior to polling the function, ensuring that there is not a chance of a race
    /// where the condition occurs AFTER checking but BEFORE registering interest
    /// in the [`WaitCell`], which could lead to deadlock.
    ///
    /// This is intended to have similar behavior to `Condvar` in the standard library,
    /// but asynchronous, and not requiring operating system intervention (or existence).
    ///
    /// In particular, this can be used in cases where interrupts or events are used
    /// to signify readiness or completion of some task, such as the completion of a
    /// DMA transfer, or reception of an ethernet frame. In cases like this, the interrupt
    /// can wake the cell, allowing the polling function to check status fields for
    /// partial progress or completion.
    ///
    /// Consider using [`Self::wait_for_value()`] if your function does return a value.
    ///
    // Consider using [`WaitQueue::wait_for()`](super::wait_queue::WaitQueue::wait_for)
    // if you need multiple waiters.
    ///
    /// # Errors
    ///
    /// Returns [`Err`]`(`[`Closed`]`)` if the [`WaitCell`] is closed.
    pub async fn wait_for<F: FnMut() -> bool>(&self, mut f: F) -> Result<(), Closed> {
        loop {
            let wait = self.subscribe().await;
            if f() {
                return Ok(());
            }
            wait.await?;
        }
    }

    /// Asynchronously poll the given function `f` until a condition occurs,
    /// using the [`WaitCell`] to only re-poll when notified.
    ///
    /// This can be used to implement a "wait loop", turning a "try" function
    /// (e.g. "try_recv" or "try_send") into an asynchronous function (e.g.
    /// "recv" or "send").
    ///
    /// In particular, this function correctly *registers* interest in the [`WaitCell`]
    /// prior to polling the function, ensuring that there is not a chance of a race
    /// where the condition occurs AFTER checking but BEFORE registering interest
    /// in the [`WaitCell`], which could lead to deadlock.
    ///
    /// This is intended to have similar behavior to `Condvar` in the standard library,
    /// but asynchronous, and not requiring operating system intervention (or existence).
    ///
    /// In particular, this can be used in cases where interrupts or events are used
    /// to signify readiness or completion of some task, such as the completion of a
    /// DMA transfer, or reception of an ethernet frame. In cases like this, the interrupt
    /// can wake the cell, allowing the polling function to check status fields for
    /// partial progress or completion, and also return the status flags at the same time.
    ///
    /// Consider using [`Self::wait_for()`] if your function does not return a value.
    ///
    // Consider using [`WaitQueue::wait_for_value()`](super::wait_queue::WaitQueue::wait_for_value) if you need multiple waiters.
    ///
    /// # Errors
    ///
    /// Returns [`Err`]`(`[`Closed`]`)` if the [`WaitCell`] is closed.
    pub async fn wait_for_value<T, F: FnMut() -> Option<T>>(&self, mut f: F) -> Result<T, Closed> {
        loop {
            let wait = self.subscribe().await;
            if let Some(t) = f() {
                return Ok(t);
            }
            wait.await?;
        }
    }

    #[tracing::instrument]
    fn take_waker(&self, close: bool) -> Option<Waker> {
        // Set the WAKING bit (to indicate that we're touching the waker) and
        // the WOKEN bit (to indicate that we intend to wake it up).
        let state = {
            let mut bits = State::WAKING | State::WOKEN;
            if close {
                bits.0 |= State::CLOSED.0;
            }
            self.fetch_or(bits, Ordering::AcqRel)
        };

        // Is anyone else touching the waker?
        if !state.contains(State::WAKING | State::REGISTERING | State::CLOSED) {
            // Safety: No one else is touching the waker right now, so it is safe to access it
            // mutably
            let waker = self.waker.with_mut(|waker| unsafe { (*waker).take() });

            // Release the lock.
            self.fetch_and(!State::WAKING, Ordering::Release);

            if let Some(waker) = waker {
                tracing::trace!(wait_cell = ?self, ?close, ?waker, "took_waker");
                return Some(waker);
            }
        }

        None
    }

    #[tracing::instrument]
    fn replace_waker(&self, waker: Waker) -> Option<Waker> {
        // Set the WAKING bit (to indicate that we're touching the waker) and
        // the WOKEN bit (to indicate that we intend to wake it up).
        let state = self.fetch_or(State::WAKING, Ordering::AcqRel);

        // Is anyone else touching the waker?
        if !state.contains(State::WAKING | State::REGISTERING | State::CLOSED) {
            // Safety: No one else is touching the waker right now, so it is safe to access it
            // mutably
            let prev_waker = self.waker.with_mut(|old_waker| unsafe {
                match &mut *old_waker {
                    Some(old_waker) if waker.will_wake(old_waker) => None,
                    old => old.replace(waker.clone()),
                }
            });

            // Release the lock.
            self.fetch_and(!State::WAKING, Ordering::Release);

            tracing::trace!(wait_cell = ?self, ?prev_waker, ?waker, "replaced_waker");
            return prev_waker;
        }

        None
    }

    #[inline(always)]
    fn compare_exchange(&self, curr: State, new: State, success: Ordering) -> Result<State, State> {
        self.state
            .0
            .compare_exchange(curr.bits(), new.bits(), success, Ordering::Acquire)
            .map(State::from_bits_retain)
            .map_err(State::from_bits_retain)
    }

    #[inline(always)]
    fn fetch_and(&self, state: State, order: Ordering) -> State {
        State::from_bits_retain(self.state.0.fetch_and(state.bits(), order))
    }

    #[inline(always)]
    fn fetch_or(&self, state: State, order: Ordering) -> State {
        State::from_bits_retain(self.state.0.fetch_or(state.bits(), order))
    }

    #[inline(always)]
    fn current_state(&self) -> State {
        State::from_bits_retain(self.state.0.load(Ordering::Acquire))
    }
}

impl Default for WaitCell {
    fn default() -> Self {
        WaitCell::new()
    }
}

impl RefUnwindSafe for WaitCell {}
impl UnwindSafe for WaitCell {}

// Safety: `WaitCell` synchronizes all accesses through atomic operations
unsafe impl Send for WaitCell {}
// Safety: `WaitCell` synchronizes all accesses through atomic operations
unsafe impl Sync for WaitCell {}

impl fmt::Debug for WaitCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WaitCell")
            .field("state", &self.current_state())
            .finish_non_exhaustive()
    }
}

impl Drop for WaitCell {
    fn drop(&mut self) {
        self.close();
    }
}

// === impl Wait ===

impl Future for Wait<'_> {
    type Output = Result<(), Closed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Did a wakeup occur while we were pre-registering the future?
        if self.presubscribe.is_ready() {
            return self.presubscribe;
        }

        // Okay, actually poll the cell, then.
        match task::ready!(self.cell.poll_wait(cx)) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(PollWaitError::Closed) => Poll::Ready(Err(Closed(()))),
            Err(PollWaitError::Busy) => {
                // If some other task was registering, yield and try to re-register
                // our waker when that task is done.
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

// === impl Subscribe ===

impl<'cell> Future for Subscribe<'cell> {
    type Output = Wait<'cell>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Pre-register the waker in the cell.
        let presubscribe = match self.cell.poll_wait(cx) {
            Poll::Ready(Err(PollWaitError::Busy)) => {
                // Someone else is in the process of registering. Yield now so we
                // can wait until that task is done, and then try again.
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            Poll::Ready(Err(PollWaitError::Closed)) => Poll::Ready(Err(Closed(()))),
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        };

        Poll::Ready(Wait {
            cell: self.cell,
            presubscribe,
        })
    }
}
