// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

pub mod steal;

use crate::loom::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use crate::scheduler::steal::{Stealer, TryStealError};
use crate::task;
use crate::task::{Header, PollResult, Task, TaskRef, TaskStub};
use alloc::boxed::Box;
use core::ptr;
use core::ptr::NonNull;
use mpsc_queue::{MpscQueue, TryDequeueError};

/// Information about the scheduler state produced after ticking.
#[derive(Debug)]
#[non_exhaustive]
pub struct Tick {
    /// `true` if the tick completed with any tasks remaining in the run queue.
    pub has_remaining: bool,

    /// The total number of tasks polled on this scheduler tick.
    pub polled: usize,

    /// The number of polled tasks that *completed* on this scheduler tick.
    ///
    /// This should always be <= `self.polled`.
    #[cfg(feature = "counters")]
    pub completed: usize,

    /// The number of tasks that were spawned since the last tick.
    #[cfg(feature = "counters")]
    pub spawned: usize,

    /// The number of tasks that were woken from outside their own `poll` calls since the last tick.
    #[cfg(feature = "counters")]
    pub woken_external: usize,

    /// The number of tasks that were woken from within their own `poll` calls during this tick.
    #[cfg(feature = "counters")]
    pub woken_internal: usize,
}

/// A scheduler that can execute tasks.
///
/// This trait defines the API required for a scheduler to handle tasks from this crate. Tasks are
/// generic over this trait so we can have multiple schedulers with different strategies. This trait
/// is not intended to be publicly implemented.
pub trait Schedule: Sized + Clone + 'static {
    /// Execute a single tick of the scheduling loop, potentially polling up to `n` tasks.
    ///
    /// This is the main logic for single-thread scheduling. It will dequeue a task, call its `poll`
    /// method, and depending on the returned [`PollResult`] mark the task as completed, or reschedule it.
    /// Much of this function is actually concerned with bookkeeping around this
    /// polling (updating the current task ptr, counting polls etc.).
    ///
    /// # Returns
    ///
    /// The returned [`Tick`] struct provides information about the executed tick, and callers should
    /// continue to tick the scheduler as long as `Tick::has_remaining` is `true`. When `Tick::has_remaining`
    /// is `false` that means the scheduler is out of tasks to actively poll and the caller should either
    /// attempt to find more tasks (e.g. by stealing from other CPU cores) or suspend the calling CPU until
    /// tasks are unblocked.
    fn tick_n(&self, n: usize) -> Tick;
    /// Attempt to steal from this `Scheduler`, the returned [`Stealer`] will grant exclusive access to
    /// steal from the `Scheduler` until it is dropped.
    ///
    /// # Errors
    ///
    /// When stealing from the `Scheduler` is not possible, either because its queue is *empty*
    /// or because there is *already an active stealer*, an error is returned.
    fn try_steal(&self) -> Result<Stealer<Self>, TryStealError>;
    /// Returns a [`TaskRef`] to the task currently being polled, or `None` if there is no active
    /// task.
    #[must_use]
    fn current_task(&self) -> Option<TaskRef>;
    /// Schedule a new task on this scheduler.
    ///
    /// This method will be called with tasks that have never been polled before (because they
    /// just got created) to allow special accounting on the scheduler side.
    fn spawn(&self, task: TaskRef);
    /// Reschedule (wake) a task on this scheduler.
    ///
    /// This method will be called by [`Waker`]s when a task needs to be rescheduled. Rescheduling
    /// will happen for tasks that returned [`Poll::Pending`] when they have signalled that they
    /// might be able to make progress again.
    fn wake(&self, task: TaskRef);
}

/// A statically-initialized scheduler implementation.
///
/// This implementation is very lightweight as it doesn't need reference counting or heap allocation,
/// but handing out `&'static` references to spawned tasks. Therefore this type *must* be stored in
/// a `static` variable (or leaked through [`Box::leak`], but like, just use [`Scheduler`] instead then).
#[derive(Debug)]
pub struct Scheduler {
    run_queue: MpscQueue<Header>,
    queued: AtomicUsize,
    current_task: AtomicPtr<Header>,
    #[cfg(feature = "counters")]
    spawned: AtomicUsize,
    #[cfg(feature = "counters")]
    woken: AtomicUsize,
}

// === impl Scheduler ===

impl Schedule for &'static Scheduler {
    fn tick_n(&self, n: usize) -> Tick {
        tracing::trace!("tick_n({self:p}, {n})");

        let mut tick = Tick {
            has_remaining: false,
            polled: 0,
            #[cfg(feature = "counters")]
            completed: 0,
            #[cfg(feature = "counters")]
            spawned: 0,
            #[cfg(feature = "counters")]
            woken_external: 0,
            #[cfg(feature = "counters")]
            woken_internal: 0,
        };

        while tick.polled < n {
            let task = match self.run_queue.try_dequeue() {
                Ok(task) => task,
                // If inconsistent, just try again.
                Err(TryDequeueError::Inconsistent) => {
                    tracing::trace!("scheduler queue {:?} inconsistent", self.run_queue);
                    core::hint::spin_loop();
                    continue;
                }
                // Queue is empty or busy (in use by something else), bail out.
                Err(TryDequeueError::Busy | TryDequeueError::Empty) => {
                    tracing::trace!("scheduler queue {:?} busy or empty", self.run_queue);
                    break;
                }
            };

            self.queued.fetch_sub(1, Ordering::SeqCst);

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
                PollResult::Ready | PollResult::ReadyJoined => {
                    #[cfg(feature = "counters")]
                    {
                        tick.completed += 1;
                    }
                }
                PollResult::PendingSchedule => {
                    self.schedule(task);
                    #[cfg(feature = "counters")]
                    {
                        tick.woken_internal += 1;
                    }
                }
                PollResult::Pending => {}
            }

            #[cfg(not(feature = "counters"))]
            tracing::trace!(poll = ?poll_result, tick.polled);
            #[cfg(feature = "counters")]
            tracing::trace!(poll = ?poll_result, tick.polled, tick.completed);
        }

        #[cfg(feature = "counters")]
        {
            tick.spawned = self.spawned.swap(0, Ordering::Relaxed);
            tick.woken_external = self.woken.swap(0, Ordering::Relaxed);
        }

        // are there still tasks in the queue? if so, we have more tasks to poll.
        if self.queued.load(Ordering::SeqCst) > 0 {
            tick.has_remaining = true;
        }

        if tick.polled > 0 {
            // log scheduler metrics.
            #[cfg(not(feature = "counters"))]
            tracing::debug!(tick.polled, tick.has_remaining,);

            #[cfg(feature = "counters")]
            tracing::debug!(
                tick.polled,
                tick.has_remaining,
                tick.completed,
                #[cfg(feature = "counters")]
                tick.spawned,
                #[cfg(feature = "counters")]
                tick.woken = tick.woken_external + tick.woken_internal,
                #[cfg(feature = "counters")]
                tick.woken.external = tick.woken_external,
                #[cfg(feature = "counters")]
                tick.woken.internal = tick.woken_internal,
            );
        }

        tick
    }

    fn try_steal(&self) -> Result<Stealer<Self>, TryStealError> {
        Stealer::new(&self.run_queue, &self.queued)
    }

    fn current_task(&self) -> Option<TaskRef> {
        let ptr = self.current_task.load(Ordering::Acquire);
        Some(TaskRef::clone_from_raw(NonNull::new(ptr)?))
    }

    fn spawn(&self, task: TaskRef) {
        #[cfg(feature = "counters")]
        self.spawned.fetch_add(1, Ordering::Relaxed);
        self.schedule(task);
    }

    fn wake(&self, task: TaskRef) {
        #[cfg(feature = "counters")]
        self.woken.fetch_add(1, Ordering::Relaxed);
        self.schedule(task);
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        let stub_task = Box::new(Task::new_stub());
        let (stub_task, _) =
            TaskRef::new_allocated::<task::Stub, task::Stub, alloc::alloc::Global>(stub_task);

        Self {
            // Safety: ensured by caller
            run_queue: MpscQueue::new_with_stub(stub_task),
            queued: AtomicUsize::new(0),
            current_task: AtomicPtr::new(ptr::null_mut()),
            #[cfg(feature = "counters")]
            spawned: AtomicUsize::new(0),
            #[cfg(feature = "counters")]
            woken: AtomicUsize::new(0),
        }
    }

    /// Construct a new `Scheduler` with *statically allocated* lock-free mpsc queue stub node.
    ///
    /// This constructor is `const` and doesn't require a heap allocation, but imposes a few
    /// awkward and nuanced restrictions on callers (therefore the `unsafe`). See
    /// [`new_scheduler`] for a safe way to construct this type.
    ///
    /// # Safety
    ///
    /// The `&'static TaskStub` reference MUST only be used for *this* constructor and **never**
    /// reused for the entire time that `Scheduler` exists.
    #[cfg(not(loom))]
    #[must_use]
    pub const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
        Self {
            // Safety: ensured by caller
            run_queue: unsafe { MpscQueue::new_with_static_stub(&stub.header) },
            queued: AtomicUsize::new(0),
            current_task: AtomicPtr::new(ptr::null_mut()),
            #[cfg(feature = "counters")]
            spawned: AtomicUsize::new(0),
            #[cfg(feature = "counters")]
            woken: AtomicUsize::new(0),
        }
    }

    fn schedule(&self, task: TaskRef) {
        self.queued.fetch_add(1, Ordering::SeqCst);
        self.run_queue.enqueue(task);
    }
}

/// Constructs a new [`Scheduler`] in a safe way.
#[cfg(not(loom))]
#[macro_export]
macro_rules! new_scheduler {
    () => {{
        static STUB: $crate::task::TaskStub = $crate::task::TaskStub::new();

        // Safety: The intrusive MPSC queue that holds tasks uses a stub node as the initial element of the
        // queue. Being intrusive, the stub can only ever be part of one collection, never multiple.
        // As such, if we were to reuse the stub node it would in effect be unlinked from the previous
        // queue. Which, unlocks a new world of fancy undefined behaviour, but unless you're into that
        // not great.
        // By defining the static above inside this block we guarantee the stub cannot escape
        // and be used elsewhere thereby solving this problem.
        unsafe { $crate::scheduler::Scheduler::new_with_static_stub(&STUB) }
    }};
}
