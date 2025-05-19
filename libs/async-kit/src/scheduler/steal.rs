// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::{AtomicUsize, Ordering};
use crate::scheduler::{Scheduler, StaticScheduler};
use crate::task;
use crate::task::TaskStub;
use crate::task::{Header, Schedule, Task, TaskBuilder, TaskRef};
use alloc::boxed::Box;
use core::marker::PhantomData;
use core::num::{NonZero, NonZeroUsize};
use mpsc_queue::MpscQueue;
use util::loom_const_fn;

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
pub struct Injector<S> {
    run_queue: MpscQueue<Header>,
    queued: AtomicUsize,
    // the correct implementation of the stealing - in particular the scheduler binding part - depends
    // on the shape of the source and destination scheduler being the same. We propagate the type through
    // the hierarchy to make it harder to fuck this up.
    _scheduler: PhantomData<S>,
}

impl<S> Default for Injector<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Injector<S> {
    pub fn new() -> Self {
        let stub_task = Box::new(Task::new_stub());
        let (stub_task, _) =
            TaskRef::new_allocated::<task::Stub, task::Stub, alloc::alloc::Global>(stub_task);

        Self {
            run_queue: MpscQueue::new_with_stub(stub_task),
            queued: AtomicUsize::new(0),
            _scheduler: PhantomData,
        }
    }

    loom_const_fn! {
        pub const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
            Self {
                // Safety: ensured by caller
                run_queue: unsafe { MpscQueue::new_with_static_stub(&stub.header) },
                queued: AtomicUsize::new(0),
                _scheduler: PhantomData,
            }
        }
    }

    /// Attempt to steal from this `Injector`, the returned [`Stealer`] will grant exclusive access to
    /// steal from the `Injector` until it is dropped.
    ///
    /// # Errors
    ///
    /// When stealing from the target is not possible, either because its queue is *empty*
    /// or because there is *already an active stealer*, an error is returned.
    pub fn try_steal(&self) -> Result<Stealer<S>, TryStealError> {
        Stealer::new(&self.run_queue, &self.queued)
    }

    #[must_use]
    #[inline]
    pub fn build_task(&self) -> TaskBuilder<S> {
        TaskBuilder::new_for_injector(self)
    }

    pub fn push_task(&self, task: TaskRef) {
        self.queued.fetch_add(1, Ordering::Relaxed);
        self.run_queue.enqueue(task);
    }
}

pub struct Stealer<'queue, S> {
    queue: mpsc_queue::Consumer<'queue, Header>,
    tasks: &'queue AtomicUsize,
    /// The initial task count in the target queue when this `Stealer` was created.
    task_snapshot: NonZeroUsize,
    // the correct implementation of the stealing - in particular the scheduler binding part - depends
    // on the shape of the source and destination scheduler being the same. We propagate the type through
    // the hierarchy to make it harder to fuck this up.
    _scheduler: PhantomData<S>,
}

impl<'a, S> Stealer<'a, S> {
    fn new(queue: &'a MpscQueue<Header>, tasks: &'a AtomicUsize) -> Result<Self, TryStealError> {
        let queue = queue.try_consume().ok_or(TryStealError::Busy)?;

        let task_snapshot =
            NonZeroUsize::new(tasks.load(Ordering::Acquire)).ok_or(TryStealError::Empty)?;

        Ok(Self {
            queue,
            tasks,
            task_snapshot,
            _scheduler: PhantomData,
        })
    }

    pub fn initial_task_count(&self) -> NonZeroUsize {
        self.task_snapshot
    }

    /// Steal a task from the queue and spawn it on the provided
    /// `scheduler`. Returns `true` when a task got successfully stolen
    /// and `false` if queue was empty.
    fn try_spawn_one(&self, scheduler: &S) -> bool
    where
        S: Schedule,
    {
        let Some(task) = self.queue.dequeue() else {
            return false;
        };

        tracing::trace!(?task, "stole");

        // decrement the target queue's task count
        self.tasks.fetch_sub(1, Ordering::Release);

        // we're moving the task to a different scheduler so we need to
        // bind to it
        // Safety: the generics ensure this is always the right type
        unsafe {
            task.bind_scheduler(scheduler.clone());
        }

        scheduler.wake(task);

        true
    }

    /// Steal a task from the queue and spawn it on the provided
    /// `scheduler`.
    ///
    /// Note this will always steal at least one task.
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
    pub fn spawn_one(&self, scheduler: &S)
    where
        S: Schedule,
    {
        assert!(
            self.try_spawn_one(scheduler),
            "Stealer target should always hold at least one task"
        );
    }

    /// Steal up to `max` task from the queue and spawn them on the provided
    /// `scheduler`.
    ///
    /// Note this will always steal at least one task.
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
    pub fn spawn_n(&self, scheduler: &S, max: NonZeroUsize) -> NonZeroUsize
    where
        S: Schedule,
    {
        let mut stolen = 0;
        while stolen <= max.get() && self.try_spawn_one(scheduler) {
            self.spawn_one(scheduler);
            stolen += 1;
        }

        NonZeroUsize::new(stolen).expect("spawn_n stole 0 tasks, this is a bug")
    }

    /// Steal half the tasks in the current queue and spawn them on the provided
    /// `scheduler`.
    ///
    /// Note this will always steal at least one task.
    pub fn spawn_half(&self, scheduler: &S) -> NonZeroUsize
    where
        S: Schedule,
    {
        // Safety: div_ceil can never return 0
        let max = unsafe { NonZero::new_unchecked(self.task_snapshot.get().div_ceil(2)) };
        self.spawn_n(scheduler, max)
    }
}

impl StaticScheduler {
    /// Attempt to steal from this `StaticScheduler`, the returned [`Stealer`] will grant exclusive access to
    /// steal from the `StaticScheduler` until it is dropped.
    ///
    /// # Errors
    ///
    /// When stealing from the `StaticScheduler` is not possible, either because its queue is *empty*
    /// or because there is *already an active stealer*, an error is returned.
    pub fn try_steal(&'static self) -> Result<Stealer<'static, &'static Self>, TryStealError> {
        Stealer::new(&self.core.run_queue, &self.core.queued)
    }
}

impl Scheduler {
    /// Attempt to steal from this `Scheduler`, the returned [`Stealer`] will grant exclusive access to
    /// steal from the `Scheduler` until it is dropped.
    ///
    /// # Errors
    ///
    /// When stealing from the `Scheduler` is not possible, either because its queue is *empty*
    /// or because there is *already an active stealer*, an error is returned.
    pub fn try_steal(&self) -> Result<Stealer<Self>, TryStealError> {
        Stealer::new(&self.core.run_queue, &self.core.queued)
    }
}
