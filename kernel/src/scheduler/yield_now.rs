// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use crate::scheduler2::scheduler;

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

            scheduler().shared.tls.get().unwrap().defer(cx.waker());

            Poll::Pending
        }
    }

    YieldNow { yielded: false }.await;
}
