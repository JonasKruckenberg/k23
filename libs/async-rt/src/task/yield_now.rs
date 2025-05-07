// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

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

#[cfg(test)]
mod tests {
    use crate::task::{yield_now, Builder, PollResult};
    use crate::tests::NopScheduler;

    #[test]
    fn basically_works() {
        let task = Builder::new(NopScheduler).build(async {
            yield_now().await;
        });
        assert!(!task.is_complete(), "newly created task can't be complete");

        // yield_now causes the poll to return Pending, but immediately reschedule
        assert!(matches!(task.poll(), PollResult::PendingSchedule));

        // yield_now only returns pending once
        assert!(matches!(task.poll(), PollResult::Ready));
    }

    #[test]
    fn multiple_yields() {
        let n = 10;

        let task = Builder::new(NopScheduler).build(async {
            for _ in 0..n {
                yield_now().await;
            }
        });
        assert!(!task.is_complete(), "newly created task can't be complete");

        // the task should return pending exactly n times
        for _ in 0..n {
            assert!(matches!(task.poll(), PollResult::PendingSchedule));
        }

        // after n times it should be done
        assert!(matches!(task.poll(), PollResult::Ready));
    }
}
