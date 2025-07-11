// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use pin_project::pin_project;

use crate::time::{
    TimeError, Timer,
    instant::Instant,
    sleep::{Sleep, sleep, sleep_until},
};
use core::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

/// Requires a `Future` to complete before the specified duration has elapsed.
///
/// # Errors
///
/// This function fails for two reasons:
/// 1. [`TimeError::NoGlobalTimer`] No global timer has been set up yet. Call [`crate::time::set_global_timer`] first.
/// 2. [`TimeError::DurationTooLong`] The requested deadline lies too far into the future
pub fn timeout<F>(
    timer: &Timer,
    duration: Duration,
    future: F,
) -> Result<Timeout<'_, F::IntoFuture>, TimeError>
where
    F: IntoFuture,
{
    Ok(Timeout {
        sleep: sleep(timer, duration)?,
        future: future.into_future(),
    })
}

/// Requires a `Future` to complete before the specified deadline has been reached.
///
/// # Errors
///
/// This function fails for two reasons:
/// 1. [`TimeError::NoGlobalTimer`] No global timer has been set up yet. Call [`crate::time::set_global_timer`] first.
/// 2. [`TimeError::DurationTooLong`] The requested deadline lies too far into the future
pub fn timeout_at<F>(
    timer: &Timer,
    deadline: Instant,
    future: F,
) -> Result<Timeout<'_, F::IntoFuture>, TimeError>
where
    F: IntoFuture,
{
    Ok(Timeout {
        sleep: sleep_until(timer, deadline)?,
        future: future.into_future(),
    })
}

/// Future returned by [`timeout`] and [`timeout_at`].
#[pin_project]
#[must_use = "futures do nothing unless `.await`ed or `poll`ed"]
pub struct Timeout<'timer, F> {
    #[pin]
    sleep: Sleep<'timer>,
    #[pin]
    future: F,
}

#[derive(Debug)]
pub struct Elapsed(());

impl<F> Timeout<'_, F> {
    /// Gets a reference to the underlying future in this timeout.
    pub fn get_ref(&self) -> &F {
        &self.future
    }

    /// Consumes this timeout, returning the underlying future.
    pub fn get_mut(&mut self) -> &mut F {
        &mut self.future
    }

    /// Consumes this timeout, returning the underlying future.
    pub fn into_inner(self) -> F {
        self.future
    }
}

impl<F: Future> Future for Timeout<'_, F> {
    type Output = Result<F::Output, Elapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let me = self.project();

        if let Poll::Ready(v) = me.future.poll(cx) {
            return Poll::Ready(Ok(v));
        }

        match me.sleep.poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(Elapsed(()))),
            Poll::Pending => Poll::Pending,
        }
    }
}
