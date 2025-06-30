// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::Ordering;
use crate::time::Ticks;
use crate::time::{TimeError, Timer, instant::Instant, timer::Entry};
use core::{
    fmt,
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};
use pin_project::{pin_project, pinned_drop};

pub fn sleep(timer: &Timer, duration: Duration) -> Result<Sleep, TimeError> {
    let ticks = timer.duration_to_ticks(duration)?;
    let now = timer.now();
    let deadline = Ticks(ticks.0 + now.0);

    Sleep::new(timer, deadline)
}

pub fn sleep_until(timer: &Timer, deadline: Instant) -> Result<Sleep, TimeError> {
    let deadline = deadline.as_ticks(timer)?;

    Sleep::new(timer, deadline)
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
    #[pin]
    entry: Entry,
}

impl<'timer> Sleep<'timer> {
    pub fn new(timer: &'timer Timer, deadline: Ticks) -> Result<Self, TimeError> {
        Ok(Self {
            state: State::Unregistered,
            timer,
            entry: Entry::new(deadline),
        })
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
            .field("state", &state)
            .field_with("addr", |f| fmt::Pointer::fmt(&entry, f))
            .field_with("timer", |f| fmt::Pointer::fmt(timer, f))
            .finish()
    }
}

impl Future for Sleep<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        tracing::trace!(self=?self, "Sleep::poll");

        let mut me = self.as_mut().project();

        match me.state {
            State::Unregistered => {
                let poll = me.timer.register(me.entry.as_mut());

                match poll {
                    Poll::Ready(()) => {
                        *me.state = State::Completed;
                        return Poll::Ready(());
                    }
                    Poll::Pending => {
                        *me.state = State::Registered;
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
        let me = self.project();
        // we only need to remove the sleep from the timer wheel if it's
        // currently part of a linked list --- if the future hasn't been polled
        // yet, or it has already completed, we don't need to lock the timer to
        // remove it.
        if me.entry.is_registered.load(Ordering::Acquire) {
            me.timer.cancel(me.entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom::sync::atomic::{AtomicBool, Ordering};
    use crate::time::test_util::MockClock;
    use crate::{
        executor::{Executor, Worker},
        loom,
    };
    use fastrand::FastRand;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt::format::FmtSpan;

    // loom is not happy about this test. For whatever reason it triggers the "too many branches"
    // error. But both the regular test AND miri are fine with it
    #[test]
    #[cfg(not(loom))]
    fn sleep_basically_works() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
            .with_thread_ids(true)
            .try_init();

        loom::model(move || {
            loom::lazy_static! {
                static ref EXEC: Executor = Executor::new();
                static ref TIMER: Timer = Timer::new(Duration::from_millis(1), MockClock::new_1us());
                static ref CALLED: AtomicBool = AtomicBool::new(false);
            }

            let mut worker = Worker::new(&EXEC, FastRand::from_seed(0));

            let th = EXEC
                .try_spawn(async move {
                    tracing::trace!("going to sleep");
                    sleep(&TIMER, Duration::from_millis(500)).unwrap().await;
                    tracing::trace!("sleep done");

                    CALLED.store(true, Ordering::Release);

                    tracing::info!("sleep done");
                })
                .unwrap();

            let clock = unsafe { TIMER.clock().data().cast::<MockClock>().as_ref().unwrap() };

            // Tick 1:
            //  During this tick the task should register itself with the timer
            //  and return Poll::Pending.
            let tick = worker.tick();
            assert_eq!(tick.polled, 1); // we polled one task
            assert_eq!(tick.completed, 0); // that task signaled it is not ready yet

            let (expired, next_deadline) = TIMER.turn();
            assert_eq!(expired, 0); // no tasks should be expired yet, clock is still at 0ms
            assert!(next_deadline.is_some()); // we should get a deadline to wait until
            assert!(!th.is_complete()); // and the task shouldn't be complete yet, since 500ms haven't elapsed yet

            // move the clock along by 250 milliseconds
            clock.advance(Duration::from_millis(250));

            // Tick 2:
            //  We expect nothing to happen during this tick, since the task is still not ready yet
            //  (we're only at 250ms not 500ms).
            let tick = worker.tick();
            assert_eq!(tick.polled, 0); // the task isn't in the workers queue, so no tasks should be polled
            assert_eq!(tick.completed, 0); // and also no tasks completed of course

            // turning the timer here should produce the same result: nothing should happen
            let (expired, next_deadline) = TIMER.turn();
            assert_eq!(expired, 0);
            assert!(next_deadline.is_some());
            assert!(!th.is_complete());

            // advance the clock another 250 milliseconds (the task should be ready now!)
            clock.advance(Duration::from_millis(250));

            // Tick 3:
            //  Polling right now should still technically do nothing, since the timer hasn't been
            //  turned yet, and therefore the task hasn't been woken.
            let tick = worker.tick();
            assert_eq!(tick.polled, 0);
            assert_eq!(tick.completed, 0);

            // turning the timer now should wake the task!
            let (expired, next_deadline) = TIMER.turn();
            assert_eq!(expired, 1); // the timer entry should have expired and the task should be woken
            assert!(next_deadline.is_none()); // consequently no deadline to wait for anymore

            // Tick 4:
            //  Polling now should complete the task since we're at 500ms AND we turned the timer
            //  to wake the task!
            let tick = worker.tick();
            assert_eq!(tick.polled, 1); // we expect the task to have been in the work queue
            assert_eq!(tick.completed, 1); // and it should be ready now
            assert!(th.is_complete()); // the JoinHandle ought to agree

            // and finally to double-check, the static should also agree
            assert!(CALLED.load(Ordering::Acquire));
        });
    }

    // #[test]
    // fn sleep_block_on() {
    //     let _trace = tracing_subscriber::fmt()
    //         .with_env_filter(EnvFilter::from_default_env())
    //         .with_thread_ids(true)
    //         .set_default();

    //     loom::model(|| {
    //         loom::lazy_static! {
    //             static ref EXEC: Executor<StdPark> = Executor::new(1, std_clock!());
    //         }

    //         let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

    //         worker.block_on(async {
    //             let begin = ::std::time::Instant::now();

    //             sleep(EXEC.timer(), Duration::from_millis(500))
    //                 .unwrap()
    //                 .await;

    //             let elapsed = begin.elapsed();
    //             assert!(
    //                 elapsed.as_millis() >= 500 && elapsed.as_millis() <= 600,
    //                 "expected to sleep between 500ms and 600ms, but got {}",
    //                 elapsed.as_millis()
    //             );
    //         });
    //     })
    // }

    // #[test]
    // fn sleep_multi_threaded() {
    //     let _trace = tracing_subscriber::fmt()
    //         .with_env_filter(EnvFilter::from_default_env())
    //         .with_thread_ids(true)
    //         .set_default();
    //
    //     const WORKERS: usize = 3;
    //     const TASKS: usize = 500;
    //
    //     loom::model(|| {
    //         loom::lazy_static! {
    //             static ref EXEC: Executor<StdPark> = Executor::new(WORKERS, std_clock!());
    //         }
    //
    //         let _guard = StopOnPanic::new(&EXEC);
    //
    //         let workers: Vec<_> = (0..WORKERS)
    //             .map(|id| {
    //                 loom::thread::spawn(move || {
    //                     let mut worker =
    //                         Worker::new(&EXEC, id, StdPark::for_current(), FastRand::from_seed(0));
    //
    //                     let tasks = (0..TASKS).map(|_| {
    //                         EXEC.try_spawn(async move {
    //                             let begin = ::std::time::Instant::now();
    //
    //                             sleep(EXEC.timer(), Duration::from_millis(500))
    //                                 .unwrap()
    //                                 .await;
    //
    //                             let elapsed = begin.elapsed();
    //                             assert!(
    //                                 elapsed.as_millis() >= 500 && elapsed.as_millis() <= 600,
    //                                 "expected to sleep between 500ms and 600ms, but got {}",
    //                                 elapsed.as_millis()
    //                             );
    //                         })
    //                         .unwrap()
    //                     });
    //
    //                     worker
    //                         .block_on(futures::future::try_join_all(tasks))
    //                         .unwrap();
    //                 })
    //             })
    //             .collect();
    //
    //         for h in workers {
    //             h.join().unwrap();
    //         }
    //     });
    // }
}
