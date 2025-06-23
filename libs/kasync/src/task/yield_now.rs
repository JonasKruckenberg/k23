// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// Yields execution to back to the runtime.
pub async fn yield_now() {
    /// Yield implementation
    struct YieldNow {
        yielded: bool,
    }

    impl Future for YieldNow {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            // ready!(crate::trace::trace_leaf(cx));

            if self.yielded {
                return Poll::Ready(());
            }

            self.yielded = true;

            // Yielding works by immediately calling `wake_by_ref` which will reinsert this
            // task into the queue (essentially a reschedule) and then returning `Poll::Pending`
            // to signal that this task is not done yet
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    YieldNow { yielded: false }.await;
}
