// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::Ordering;
use crate::time::timer::Entry;
use crate::time::{Instant, Ticks, TimeError, Timer};
use core::fmt;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, ready};
use core::time::Duration;
use pin_project::{pin_project, pinned_drop};

/// Wait until duration has elapsed.
///
/// # Errors
///
/// This function fails for two reasons:
/// 1. [`TimeError::NoGlobalTimer`] No global timer has been set up yet. Call [`crate::time::set_global_timer`] first.
/// 2. [`TimeError::DurationTooLong`] The requested duration is too big
pub fn sleep(timer: &Timer, duration: Duration) -> Result<Sleep, TimeError> {
    let ticks = timer.clock.duration_to_ticks(duration)?;

    Ok(Sleep::new(timer, ticks))
}

/// Wait until the deadline has been reached.
///
/// # Errors
///
/// This function fails for two reasons:
/// 1. [`TimeError::NoGlobalTimer`] No global timer has been set up yet. Call [`crate::time::set_global_timer`] first.
/// 2. [`TimeError::DurationTooLong`] The requested deadline lies too far into the future
pub fn sleep_until(timer: &Timer, deadline: Instant) -> Result<Sleep, TimeError> {
    let now = timer.clock.now();
    let duration = deadline.duration_since(now);
    let ticks = timer.clock.duration_to_ticks(duration)?;

    Ok(Sleep::new(timer, ticks))
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum State {
    Unregistered,
    Registered,
    Completed,
}

/// Future returned by [`sleep`] and [`sleep_until`].
#[pin_project(PinnedDrop)]
#[must_use = "futures do nothing unless `.await`ed or `poll`ed"]
pub struct Sleep<'timer> {
    state: State,
    timer: &'timer Timer,
    ticks: Ticks,
    #[pin]
    entry: Entry,
}

impl<'timer> Sleep<'timer> {
    fn new(timer: &'timer Timer, ticks: Ticks) -> Self {
        let now = timer.clock.now_ticks();
        let deadline = Ticks(now.0 + ticks.0);

        Self {
            state: State::Unregistered,
            timer,
            ticks,
            entry: Entry::new(deadline),
        }
    }
}

impl Future for Sleep<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        tracing::trace!(self=?self, "Sleep::poll");
        let mut me = self.as_mut().project();

        match me.state {
            State::Unregistered => {
                let mut lock = me.timer.core.lock();

                // While we are holding the wheel lock, go ahead and advance the
                // timer, too. This way, the timer wheel gets advanced more
                // frequently than just when a scheduler tick completes or a
                // timer IRQ fires, helping to increase timer accuracy.
                me.timer.turn_locked(&mut lock);

                // Safety: the timer impl promises to treat the pointer as pinned
                let ptr = unsafe { NonNull::from(Pin::into_inner_unchecked(me.entry.as_mut())) };

                // Safety: we just created the pointer from a mutable reference
                match unsafe { lock.register(ptr) } {
                    Poll::Ready(()) => {
                        *me.state = State::Completed;
                        return Poll::Ready(());
                    }
                    Poll::Pending => {
                        *me.state = State::Registered;
                        drop(lock);
                    }
                }
            }
            State::Registered if me.entry.is_registered.load(Ordering::Acquire) => {}
            _ => return Poll::Ready(()),
        }

        let _poll = ready!(me.entry.waker.poll_wait(cx));
        debug_assert!(
            _poll.is_err(),
            "a Sleep's WaitCell should only be woken by closing"
        );
        Poll::Ready(())
    }
}

#[pinned_drop]
impl PinnedDrop for Sleep<'_> {
    fn drop(mut self: Pin<&mut Self>) {
        tracing::trace!("Sleep::drop");
        let this = self.project();
        // we only need to remove the sleep from the timer wheel if it's
        // currently part of a linked list --- if the future hasn't been polled
        // yet, or it has already completed, we don't need to lock the timer to
        // remove it.
        if this.entry.is_registered.load(Ordering::Acquire) {
            let mut lock = this.timer.core.lock();
            lock.cancel(this.entry);
        }
    }
}

impl fmt::Debug for Sleep<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            state,
            entry,
            timer,
            ..
        } = self;
        f.debug_struct("Sleep")
            .field("duration", &self.timer.clock.ticks_to_duration(self.ticks))
            .field("state", &state)
            .field_with("addr", |f| fmt::Pointer::fmt(&entry, f))
            .field_with("timer", |f| fmt::Pointer::fmt(timer, f))
            .finish()
    }
}
