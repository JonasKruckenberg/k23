// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::time::{sleep, Sleep};
use core::future::{Future, IntoFuture};
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use pin_project::pin_project;

#[expect(tail_expr_drop_order, reason = "")]
pub fn timeout<F>(duration: Duration, future: F) -> Timeout<'static, F::IntoFuture>
where
    F: IntoFuture,
{
    Timeout {
        future: future.into_future(),
        sleep: sleep(duration),
    }
}

#[pin_project]
pub struct Timeout<'timer, F> {
    #[pin]
    future: F,
    #[pin]
    sleep: Sleep<'timer>,
}

#[derive(Debug)]
pub struct Elapsed;

impl<F> Future for Timeout<'_, F>
where
    F: Future,
{
    type Output = Result<F::Output, Elapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        tracing::trace!("Timeout::Poll");
        let me = self.project();

        if let Poll::Ready(v) = me.future.poll(cx) {
            return Poll::Ready(Ok(v));
        }

        match me.sleep.poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(Elapsed)),
            Poll::Pending => Poll::Pending,
        }
    }
}
