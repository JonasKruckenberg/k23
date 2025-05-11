// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod steal;

use crate::loom::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use crate::task;
use crate::task::{Header, JoinHandle, PollResult, Schedule, Task, TaskBuilder, TaskRef, TaskStub};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::alloc::{AllocError, Allocator};
use core::ptr;
use core::ptr::NonNull;
use mpsc_queue::{MpscQueue, TryDequeueError};
use util::loom_const_fn;

pub use steal::{Injector, Stealer, TryStealError};

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
    pub completed: usize,

    /// The number of tasks that were spawned since the last tick.
    pub spawned: usize,

    /// The number of tasks that were woken from outside their own `poll` calls since the last tick.
    pub woken_external: usize,

    /// The number of tasks that were woken from within their own `poll` calls during this tick.
    pub woken_internal: usize,
}

impl Tick {
    /// Returns the total number of tasks woken since the last poll.
    pub fn woken(&self) -> usize {
        self.woken_external + self.woken_internal
    }
}

#[derive(Debug)]
struct Core {
    run_queue: MpscQueue<Header>,
    queued: AtomicUsize,
    current_task: AtomicPtr<Header>,
    spawned: AtomicUsize,
    woken: AtomicUsize,
}

/// A statically-initialized scheduler implementation.
///
/// This implementation is very lightweight as it doesn't need reference counting or heap allocation,
/// but handing out `&'static` references to spawned tasks. Therefore this type *must* be stored in
/// a `static` variable (or leaked through [`Box::leak`], but like, just use [`Scheduler`] instead then).
#[derive(Debug)]
pub struct StaticScheduler {
    core: Core,
}

/// A heap-allocated, reference-counted scheduler implementation.
///
/// This implementation has more overhead (allocation & reference counting) that [`StaticScheduler`]
/// but is also much more flexible.
#[derive(Debug, Clone)]
pub struct Scheduler {
    core: Arc<Core>,
}

// === impl Core ===

impl Core {
    const DEFAULT_TICK_SIZE: usize = 256;

    fn new() -> Self {
        let stub_task = Box::new(Task::new_stub());
        let (stub_task, _) =
            TaskRef::new_allocated::<task::Stub, task::Stub, alloc::alloc::Global>(stub_task);

        Self {
            run_queue: MpscQueue::new_with_stub(stub_task),
            queued: AtomicUsize::new(0),
            current_task: AtomicPtr::new(ptr::null_mut()),
            spawned: AtomicUsize::new(0),
            woken: AtomicUsize::new(0),
        }
    }

    loom_const_fn! {
        const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
            Self {
                // Safety: ensured by caller
                run_queue: unsafe { MpscQueue::new_with_static_stub(&stub.header) },
                queued: AtomicUsize::new(0),
                current_task: AtomicPtr::new(ptr::null_mut()),
                spawned: AtomicUsize::new(0),
                woken: AtomicUsize::new(0),
            }
        }
    }

    fn current_task(&self) -> Option<TaskRef> {
        let ptr = self.current_task.load(Ordering::Acquire);
        Some(TaskRef::clone_from_raw(NonNull::new(ptr)?))
    }

    /// Like [`Self::schedule`] but for tasks that are rescheduled by their Wakers.
    fn wake(&self, task: TaskRef) {
        self.woken.fetch_add(1, Ordering::Relaxed);
        self.schedule(task);
    }

    /// Like [`Self::schedule`] but for tasks that are scheduled for the first time.
    fn spawn(&self, task: TaskRef) {
        self.spawned.fetch_add(1, Ordering::Relaxed);
        self.schedule(task);
    }

    fn schedule(&self, task: TaskRef) {
        self.queued.fetch_add(1, Ordering::Relaxed);
        self.run_queue.enqueue(task);
    }

    fn tick_n(&self, n: usize) -> Tick {
        tracing::trace!("tick_n({n})");

        let mut tick = Tick {
            has_remaining: false,
            polled: 0,
            completed: 0,
            spawned: 0,
            woken_external: 0,
            woken_internal: 0,
        };

        while tick.polled < n {
            tracing::debug!("{:?}", self.run_queue);
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

        tick.spawned = self.spawned.swap(0, Ordering::Relaxed);
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

// === impl StaticScheduler ===

impl Schedule for &'static StaticScheduler {
    fn wake(&self, task: TaskRef) {
        self.core.wake(task);
    }

    fn spawn(&self, task: TaskRef) {
        self.core.spawn(task);
    }
}

impl StaticScheduler {
    pub const DEFAULT_TICK_SIZE: usize = Core::DEFAULT_TICK_SIZE;

    loom_const_fn! {
        /// Returns a new scheduler, suitable for being put into a `static`.
        ///
        /// # Safety
        ///
        /// You must ensure that the `&'static TaskStub` is only ever used to construct *one*
        /// scheduler, never multiple. See `new_static_scheduler` for a way to construct a
        /// `StaticScheduler` in a safe way.
        pub const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
            Self {
                // Safety: ensured by caller
                core: unsafe { Core::new_with_static_stub(stub) },
            }
        }
    }

    /// Returns a [`TaskRef`] to the task currently being polled, or `None` if there is no active
    /// task.
    #[must_use]
    #[inline]
    pub fn current_task(&'static self) -> Option<TaskRef> {
        self.core.current_task()
    }

    /// Returns a new [`TaskBuilder`] for configuring tasks prior to spawning them
    /// onto this scheduler.
    #[must_use]
    #[inline]
    pub fn build_task<'a>(&'static self) -> TaskBuilder<'a, &'static Self> {
        TaskBuilder::new(self)
    }

    /// Attempt to spawn a given [`Future`] onto this scheduler.
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output
    /// as well as control some aspects of its runtime behaviour (such as cancelling it).
    ///
    /// Spawning tasks on its own does nothing, the [`StaticScheduler`] must be run with [`Self::tick`] or [`Self::tick_n`]
    /// in order to actually make progress.
    ///
    /// If you want to configure the task before spawning it, such as overriding its name, kind, or location
    /// see [`Self::build_task`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    #[inline]
    #[track_caller]
    pub fn try_spawn<F>(&'static self, future: F) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        self.build_task().try_spawn(future)
    }

    /// Attempt to spawn a given [`Future`] onto this scheduler.
    ///
    /// Unlike `Self::try_spawn` this will attempt to allocate the task on the provided allocator
    /// instead of the default global one.
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output
    /// as well as control some aspects of its runtime behaviour (such as cancelling it).
    ///
    /// Spawning tasks on its own does nothing, the [`StaticScheduler`] must be run with [`Self::tick`] or [`Self::tick_n`]
    /// in order to actually make progress.
    ///
    /// If you want to configure the task before spawning it, such as overriding its name, kind, or location
    /// see [`Self::build_task`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    #[inline]
    #[track_caller]
    pub fn try_spawn_in<F, A>(
        &'static self,
        future: F,
        alloc: A,
    ) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + 'static,
        F::Output: 'static,
        A: Allocator,
    {
        self.build_task().try_spawn_in(future, alloc)
    }

    /// Tick this scheduler forward, polling up to [`Self::DEFAULT_TICK_SIZE`] tasks
    /// from the scheduler's run queue.
    ///
    /// Only a single CPU core/thread may tick a given scheduler at a time. If
    /// another call to `tick` is in progress on a different core, this method
    /// will immediately return.
    ///
    /// The returned `Tick` struct describes what happened during the tick and importantly
    /// if the caller should continue to call [`Self::tick`] or put the CPU/thread to sleep.
    pub fn tick(&'static self) -> Tick {
        self.core.tick_n(Self::DEFAULT_TICK_SIZE)
    }

    /// Tick this scheduler forward, polling up to `n` tasks
    /// from the scheduler's run queue.
    ///
    /// Only a single CPU core/thread may tick a given scheduler at a time. If
    /// another call to `tick` is in progress on a different core, this method
    /// will immediately return.
    ///
    /// The returned `Tick` struct describes what happened during the tick and importantly
    /// if the caller should continue to call [`Self::tick`] or put the CPU/thread to sleep.
    pub fn tick_n(&'static self, n: usize) -> Tick {
        self.core.tick_n(n)
    }
}

#[macro_export]
macro_rules! new_static_scheduler {
    () => {{
        static STUB: $crate::task::TaskStub = $crate::task::TaskStub::new();

        // Safety: TODO
        unsafe { $crate::scheduler::StaticScheduler::new_with_static_stub(&STUB) }
    }};
}

// === impl Scheduler ===

impl Schedule for Scheduler {
    fn wake(&self, task: TaskRef) {
        self.core.wake(task);
    }

    fn spawn(&self, task: TaskRef) {
        self.core.spawn(task);
    }
}

impl Scheduler {
    pub const DEFAULT_TICK_SIZE: usize = Core::DEFAULT_TICK_SIZE;

    /// Returns a new heap-allocated, and reference-counted scheduler.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Arc::new(Core::new()),
        }
    }

    /// Returns a [`TaskRef`] to the task currently being polled, or `None` if there is no active
    /// task.
    #[must_use]
    #[inline]
    pub fn current_task(&self) -> Option<TaskRef> {
        self.core.current_task()
    }

    /// Returns a new [`TaskBuilder`] for configuring tasks prior to spawning them
    /// onto this scheduler.
    #[must_use]
    pub fn build_task<'a>(&self) -> TaskBuilder<'a, Self> {
        TaskBuilder::new(self.clone())
    }

    /// Attempt to spawn a given [`Future`] onto this scheduler.
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output
    /// as well as control some aspects of its runtime behaviour (such as cancelling it).
    ///
    /// Spawning tasks on its own does nothing, the [`Scheduler`] must be run with [`Self::tick`] or [`Self::tick_n`]
    /// in order to actually make progress.
    ///
    /// If you want to configure the task before spawning it, such as overriding its name, kind, or location
    /// see [`Self::build_task`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    #[inline]
    #[track_caller]
    pub fn try_spawn<F>(&self, future: F) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        self.build_task().try_spawn(future)
    }

    /// Attempt to spawn a given [`Future`] onto this scheduler.
    ///
    /// Unlike `Self::try_spawn` this will attempt to allocate the task on the provided allocator
    /// instead of the default global one.
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output
    /// as well as control some aspects of its runtime behaviour (such as cancelling it).
    ///
    /// Spawning tasks on its own does nothing, the [`Scheduler`] must be run with [`Self::tick`] or [`Self::tick_n`]
    /// in order to actually make progress.
    ///
    /// If you want to configure the task before spawning it, such as overriding its name, kind, or location
    /// see [`Self::build_task`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    #[inline]
    #[track_caller]
    pub fn try_spawn_in<F, A>(
        &self,
        future: F,
        alloc: A,
    ) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + 'static,
        F::Output: 'static,
        A: Allocator,
    {
        self.build_task().try_spawn_in(future, alloc)
    }

    /// Tick this scheduler forward, polling up to [`Self::DEFAULT_TICK_SIZE`] tasks
    /// from the scheduler's run queue.
    ///
    /// Only a single CPU core/thread may tick a given scheduler at a time. If
    /// another call to `tick` is in progress on a different core, this method
    /// will immediately return.
    ///
    /// The returned `Tick` struct describes what happened during the tick and importantly
    /// if the caller should continue to call [`Self::tick`] or put the CPU/thread to sleep.
    pub fn tick(&self) -> Tick {
        self.core.tick_n(Self::DEFAULT_TICK_SIZE)
    }

    /// Tick this scheduler forward, polling up to `n` tasks
    /// from the scheduler's run queue.
    ///
    /// Only a single CPU core/thread may tick a given scheduler at a time. If
    /// another call to `tick` is in progress on a different core, this method
    /// will immediately return.
    ///
    /// The returned `Tick` struct describes what happened during the tick and importantly
    /// if the caller should continue to call [`Self::tick`] or put the CPU/thread to sleep.
    pub fn tick_n(&self, n: usize) -> Tick {
        self.core.tick_n(n)
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::hint::black_box;
    use core::pin::Pin;
    use core::sync::atomic::{AtomicBool, Ordering};
    use core::task::{Context, Poll, Waker};
    use spin::Mutex;
    use tracing::Level;

    #[test]
    fn static_scheduler_works() {
        static SCHED: StaticScheduler = new_static_scheduler!();
        static CALLED: AtomicBool = AtomicBool::new(false);

        let _join = SCHED
            .build_task()
            .try_spawn(async {
                CALLED.store(true, Ordering::Relaxed);
            })
            .unwrap();

        let tick = SCHED.tick();

        assert_eq!(tick.has_remaining, false);
        assert_eq!(tick.polled, 1);
        assert_eq!(tick.completed, 1);
        assert_eq!(tick.spawned, 1);
        assert_eq!(tick.woken_external, 1);
        assert_eq!(tick.woken_internal, 0);

        black_box(_join); // ensure the task lives for the entirety of the test
    }

    #[test]
    fn alloc_scheduler_works() {
        tracing_subscriber::fmt()
            .with_max_level(Level::TRACE)
            .init();

        static CALLED: AtomicBool = AtomicBool::new(false);

        let sched = Scheduler::new();

        let _join = sched
            .build_task()
            .try_spawn(async {
                CALLED.store(true, Ordering::Relaxed);
            })
            .unwrap();

        let tick = sched.tick();

        assert_eq!(tick.has_remaining, false);
        assert_eq!(tick.polled, 1);
        assert_eq!(tick.completed, 1);
        assert_eq!(tick.spawned, 1);
        assert_eq!(tick.woken_external, 1);
        assert_eq!(tick.woken_internal, 0);

        black_box(_join); // ensure the task lives for the entirety of the test
    }

    #[test]
    fn wake() {
        tracing_subscriber::fmt()
            .with_max_level(Level::TRACE)
            .init();

        static WAKER: Mutex<Option<Waker>> = Mutex::new(None);

        #[derive(Default)]
        struct Yield {
            yielded: bool,
        }
        impl Future for Yield {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                if !self.yielded {
                    *WAKER.lock() = Some(cx.waker().clone());
                    self.as_mut().yielded = true;
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            }
        }

        let sched = Scheduler::new();

        tracing::debug!("spawn");

        let _join = sched
            .build_task()
            .try_spawn(async {
                Yield::default().await;
            })
            .unwrap();

        tracing::debug!("tick 1");

        let tick = sched.tick();
        // there are still "alive" tasks, but none we can poll right now
        // at this point a runtime should go to sleep
        assert_eq!(tick.has_remaining, false);
        assert_eq!(tick.polled, 1);
        assert_eq!(tick.completed, 0);
        assert_eq!(tick.spawned, 1);
        assert_eq!(tick.woken_external, 0);
        assert_eq!(tick.woken_internal, 0);

        tracing::debug!("after tick 1");

        tracing::debug!("tick 2");

        // if we are to call tick again, nothing should be processed
        let tick = sched.tick();
        assert_eq!(tick.has_remaining, false);
        assert_eq!(tick.polled, 0);
        assert_eq!(tick.completed, 0);
        assert_eq!(tick.spawned, 0);
        assert_eq!(tick.woken_external, 0);
        assert_eq!(tick.woken_internal, 0);

        tracing::debug!("after tick 2");

        // call the tasks waker to simulate a timer or IRQ event
        WAKER.lock().take().unwrap().wake();

        tracing::debug!("tick 3");

        // now ticking should process the task again
        let tick = sched.tick();
        assert_eq!(tick.has_remaining, false);
        assert_eq!(tick.polled, 1);
        assert_eq!(tick.completed, 1);
        assert_eq!(tick.spawned, 0);
        assert_eq!(tick.woken_external, 1);
        assert_eq!(tick.woken_internal, 0);

        tracing::debug!("after tick 3");

        black_box(_join); // ensure the task lives for the entirety of the test
    }
}
