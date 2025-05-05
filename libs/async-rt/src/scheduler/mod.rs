// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::task::{Header, PollResult, Schedule, TaskPool, TaskRef, TaskStub};
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use mpsc_queue::{MpscQueue, TryDequeueError};

const DEFAULT_TICK_SIZE: usize = 256;

pub struct Tick {
    /// The total number of tasks polled on this scheduler tick.
    pub polled: usize,

    /// The number of polled tasks that *completed* on this scheduler tick.
    ///
    /// This should always be <= `self.polled`.
    pub completed: usize,

    /// `true` if the tick completed with any tasks remaining in the run queue.
    pub has_remaining: bool,

    /// The number of tasks that were woken from outside of their own `poll`
    /// calls since the last tick.
    pub woken_external: usize,

    /// The number of tasks that were woken from within their own `poll` calls
    /// during this tick.
    pub woken_internal: usize,
}

impl Tick {
    /// Returns the total number of tasks woken since the last poll.
    pub fn woken(&self) -> usize {
        self.woken_external + self.woken_internal
    }
}

#[macro_export]
macro_rules! new_scheduler {
    () => {{
        static STUB_TASK: $crate::task::TaskStub = $crate::task::TaskStub::new();
        unsafe {
            // safety: `Scheduler::new_with_static_stub` is unsafe because
            // the stub task must not be shared with any other `Scheduler`
            // instance. because the `new_static` macro creates the stub task
            // inside the scope of the static initializer, it is guaranteed that
            // no other `Scheduler` instance can reference the `STUB_TASK`
            // static, so this is always safe.
            $crate::scheduler::Scheduler::new_with_static_stub(&STUB_TASK)
        }
    }};
}

pub struct Scheduler {
    core: Core,
    task_pool: TaskPool,
}

impl Schedule for &'static Scheduler {
    fn schedule(&self, task: TaskRef) {
        self.core.schedule(task);
    }
}

impl Scheduler {
    #[doc(hidden)]
    pub const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
        // Safety: ensured by caller
        unsafe {
            Self {
                core: Core::new_with_static_stub(stub),
                task_pool: TaskPool::new(),
            }
        }
    }

    pub fn tick(&self) -> Tick {
        self.core.tick_n(DEFAULT_TICK_SIZE)
    }

    pub fn tick_n(&self, n: usize) -> Tick {
        self.core.tick_n(n)
    }

    pub(crate) fn bind(&self, task: TaskRef) -> Option<TaskRef> {
        self.task_pool.bind(task)
    }
}

struct Core {
    /// local queue of tasks to run
    run_queue: MpscQueue<Header>,
    queued: AtomicUsize,
    woken: AtomicUsize,
    current_task: AtomicPtr<Header>,
}

impl Core {
    const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
        Self {
            // Safety: ensured by caller
            run_queue: unsafe { MpscQueue::new_with_static_stub(&stub.header) },
            queued: AtomicUsize::new(0),
            woken: AtomicUsize::new(0),
            current_task: AtomicPtr::new(ptr::null_mut()),
        }
    }

    pub fn schedule(&self, task: TaskRef) {
        self.queued.fetch_add(1, Ordering::Relaxed);
        self.run_queue.enqueue(task);
    }

    pub fn tick_n(&self, n: usize) -> Tick {
        let mut tick = Tick {
            polled: 0,
            completed: 0,
            woken_external: 0,
            woken_internal: 0,
            has_remaining: false,
        };

        while tick.polled < n {
            let task = match self.run_queue.try_dequeue() {
                Ok(task) => task,
                // If inconsistent, just try again.
                Err(TryDequeueError::Inconsistent) => {
                    core::hint::spin_loop();
                    continue;
                }
                // Queue is empty or busy (in use by something else), bail out.
                Err(TryDequeueError::Busy | TryDequeueError::Empty) => {
                    break;
                }
            };

            self.queued.fetch_sub(1, Ordering::Relaxed);
            let _span = tracing::trace_span!(
                "poll",
                task.addr = ?task.header_ptr(),
                task.tid = task.id().as_u64(),
            )
            .entered();
            // store the currently polled task in the `current_task` pointer.
            // using `TaskRef::as_ptr` is safe here, since we will clear the
            // `current_task` pointer before dropping the `TaskRef`.
            self.current_task
                .store(task.header_ptr().as_ptr(), Ordering::Release);

            // poll the task
            let poll_result = task.poll();

            // clear the current task cell before potentially dropping the
            // `TaskRef`.
            self.current_task.store(ptr::null_mut(), Ordering::Release);

            tick.polled += 1;
            match poll_result {
                PollResult::Ready | PollResult::ReadyJoined => tick.completed += 1,
                PollResult::PendingSchedule => {
                    self.schedule(task);
                    tick.woken_internal += 1;
                }
                PollResult::Pending => {}
            }

            tracing::trace!(poll = ?poll_result, tick.polled, tick.completed);
        }

        tick.woken_external = self.woken.swap(0, Ordering::Relaxed);

        // are there still tasks in the queue? if so, we have more tasks to poll.
        if self.queued.load(Ordering::Relaxed) > 0 {
            tick.has_remaining = true;
        }

        if tick.polled > 0 {
            // log scheduler metrics.
            tracing::debug!(
                tick.polled,
                tick.completed,
                // tick.spawned,
                tick.woken = tick.woken(),
                tick.woken.external = tick.woken_external,
                tick.woken.internal = tick.woken_internal,
                tick.has_remaining
            );
        }

        tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Builder;

    #[test]
    fn works() {
        static SCHED: Scheduler = new_scheduler!();

        Builder::new(&SCHED)
            .try_spawn(async {
                println!("hello world");
            })
            .unwrap();

        SCHED.tick();
    }
}
