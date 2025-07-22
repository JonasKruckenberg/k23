// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::fmt::Debug;
use core::num::NonZeroUsize;

use cordyceps::{MpscQueue, mpsc_queue};

use crate::executor::Scheduler;
use crate::loom::sync::atomic::{AtomicUsize, Ordering};
use crate::task::{Header, Task, TaskRef};

#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum TryStealError {
    /// Tasks could not be stolen because the targeted queue already has a
    /// consumer.
    Busy,
    /// No tasks were available to steal.
    Empty,
}

#[derive(Debug)]
pub struct Injector {
    run_queue: MpscQueue<Header>,
    queued: AtomicUsize,
}

impl Default for Injector {
    fn default() -> Self {
        Self::new()
    }
}

impl Injector {
    pub fn new() -> Self {
        let stub_task = Box::new(Task::new_stub());
        let (stub_task, _) = TaskRef::new_allocated(stub_task);

        Self {
            run_queue: MpscQueue::new_with_stub(stub_task),
            queued: AtomicUsize::new(0),
        }
    }

    /// Attempt to steal from this `Injector`, the returned [`Stealer`] will grant exclusive access to
    /// steal from the `Injector` until it is dropped.
    ///
    /// # Errors
    ///
    /// When stealing from the target is not possible, either because its queue is *empty*
    /// or because there is *already an active stealer*, an error is returned.
    pub fn try_steal(&self) -> Result<Stealer<'_>, TryStealError> {
        Stealer::new(&self.run_queue, &self.queued)
    }

    pub fn push_task(&self, task: TaskRef) {
        self.queued.fetch_add(1, Ordering::SeqCst);
        self.run_queue.enqueue(task);
    }
}

pub struct Stealer<'queue> {
    queue: mpsc_queue::Consumer<'queue, Header>,
    tasks: &'queue AtomicUsize,
    /// The initial task count in the target queue when this `Stealer` was created.
    task_snapshot: NonZeroUsize,
}

impl<'queue> Stealer<'queue> {
    pub(crate) fn new(
        queue: &'queue MpscQueue<Header>,
        tasks: &'queue AtomicUsize,
    ) -> Result<Self, TryStealError> {
        let queue = queue.try_consume().ok_or(TryStealError::Busy)?;

        let task_snapshot = tasks.load(Ordering::SeqCst);
        let Some(task_snapshot) = NonZeroUsize::new(task_snapshot) else {
            return Err(TryStealError::Empty);
        };

        Ok(Self {
            queue,
            tasks,
            task_snapshot,
        })
    }

    /// Steal a task from the queue and spawn it on the provided
    /// `scheduler`. Returns `true` when a task got successfully stolen
    /// and `false` if queue was empty.
    pub fn spawn_one(&self, scheduler: &'static Scheduler) -> bool {
        let Some(task) = self.queue.dequeue() else {
            return false;
        };

        tracing::trace!(?task, "stole");

        // decrement the target queue's task count
        self.tasks.fetch_sub(1, Ordering::SeqCst);

        // we're moving the task to a different scheduler so we need to
        // bind to it
        task.bind_scheduler(scheduler);

        scheduler.schedule(task);

        true
    }

    /// Steal up to `max` task from the queue and spawn them on the provided
    /// `scheduler`.
    ///
    /// Note this will always steal at least one task.
    pub fn spawn_n(&self, core: &'static Scheduler, max: usize) -> usize {
        let mut stolen = 0;
        while stolen <= max && self.spawn_one(core) {
            stolen += 1;
        }
        stolen
    }

    /// Steal half the tasks in the current queue and spawn them on the provided
    /// `scheduler`.
    ///
    /// Note this will always steal at least one task.
    pub fn spawn_half(&self, core: &'static Scheduler) -> usize {
        let max = self.task_snapshot.get().div_ceil(2);
        self.spawn_n(core, max)
    }
}
