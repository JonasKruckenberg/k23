// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod steal;

use core::alloc::AllocError;
use core::num::NonZeroUsize;
use core::ptr;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use cordyceps::mpsc_queue::{MpscQueue, TryDequeueError};
use cpu_local::collection::CpuLocal;
use fastrand::FastRand;
use futures::pin_mut;
use spin::Backoff;

use crate::error::{Closed, SpawnError};
use crate::executor::steal::{Injector, Stealer, TryStealError};
use crate::future::Either;
use crate::loom::sync::atomic::{AtomicPtr, AtomicUsize};
use crate::sync::wait_queue::WaitQueue;
use crate::task::{Header, JoinHandle, PollResult, TaskBuilder, TaskRef};

#[derive(Debug)]
pub struct Executor {
    schedulers: CpuLocal<Scheduler>,
    injector: Injector,
    sleepers: WaitQueue,
}

#[derive(Debug)]
pub struct Worker {
    id: usize,
    executor: &'static Executor,
    scheduler: &'static Scheduler,
    rng: FastRand,
}

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

#[derive(Debug)]
pub struct Scheduler {
    run_queue: MpscQueue<Header>,
    current_task: AtomicPtr<Header>,
    queued: AtomicUsize,
    #[cfg(feature = "counters")]
    spawned: AtomicUsize,
    #[cfg(feature = "counters")]
    woken: AtomicUsize,
}

// === impl Executor ===

impl Executor {
    /// # Errors
    ///
    /// Returns `AllocError` when allocating the underlying resources fails.
    pub fn new() -> Result<Self, AllocError> {
        Ok(Self {
            schedulers: CpuLocal::new(),
            injector: Injector::new()?,
            sleepers: WaitQueue::new(),
        })
    }

    /// # Errors
    ///
    /// Returns `AllocError` when allocating the underlying resources fails.
    pub fn with_capacity(capacity: usize) -> Result<Self, AllocError> {
        Ok(Self {
            schedulers: CpuLocal::with_capacity(capacity),
            injector: Injector::new()?,
            sleepers: WaitQueue::new(),
        })
    }

    /// Closes the executor.
    ///
    /// After calling close all ongoing and future calls to [`Worker::run`] will return `Err(Closed)`.
    pub fn close(&self) {
        self.sleepers.close();
    }

    /// Returns `true` if the executor has been closed.
    ///
    /// The executor is closed by calling [`close`][Self::close].
    ///
    /// If true is returned, a call to send will always result in an error.
    pub fn is_closed(&self) -> bool {
        self.sleepers.is_closed()
    }

    pub fn wake_one(&self) {
        self.sleepers.wake();
    }

    pub fn current_scheduler(&self) -> Option<&Scheduler> {
        self.schedulers.get()
    }

    pub fn build_task<'a>(
        &'static self,
    ) -> TaskBuilder<'a, impl Fn(TaskRef) -> Result<(), Closed>> {
        TaskBuilder::new(|task| {
            if self.is_closed() {
                return Err(Closed(()));
            }

            if let Some(scheduler) = self.schedulers.get() {
                // we need to bind the scheduler here
                task.bind_scheduler(scheduler);

                scheduler.schedule(task);

                Ok(())
            } else {
                self.injector.push_task(task);

                Ok(())
            }
        })
    }

    /// Attempt spawn this [`Future`] onto this executor.
    ///
    /// This method returns a [`TaskRef`] which can be used to spawn it onto an [`crate::executor::Executor`]
    /// and a [`JoinHandle`] which can be used to await the futures output as well as control some aspects
    /// of its runtime behaviour (such as cancelling it).
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    pub fn try_spawn<F>(&'static self, future: F) -> Result<JoinHandle<F::Output>, SpawnError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.build_task().try_spawn(future)
    }
}

// === impl Worker ===

impl Worker {
    /// # Errors
    ///
    /// Returns `AllocError` when allocating the underlying resources fails.
    pub fn new(executor: &'static Executor, rng: FastRand) -> Result<Self, AllocError> {
        let id = executor.schedulers.len();
        let core = executor.schedulers.get_or_try(Scheduler::new)?;

        Ok(Self {
            id,
            executor,
            scheduler: core,
            rng,
        })
    }

    /// Returns a reference to the task that's current being polled or `None`.
    #[must_use]
    pub fn current_task(&self) -> Option<TaskRef> {
        self.scheduler.current_task()
    }

    /// Run the worker main loop until the given future completes.
    ///
    /// # Errors
    ///
    /// Returns `Err(Closed)` if the executor has been closed before the future completes.
    pub async fn run<F>(&mut self, future: F) -> Result<F::Output, Closed>
    where
        F: Future + Send,
        F::Output: Send,
    {
        if self.executor.is_closed() {
            return Err(Closed(()));
        }

        let main_loop = self.main_loop();
        pin_mut!(future);
        pin_mut!(main_loop);

        let res = crate::future::select(future, main_loop).await;
        match res {
            Either::Left((val, _)) => Ok(val),
            // The `main_loop` future either never returns or always returns Err(Closed)
            Either::Right((Err(err), _)) => Err(err),
        }
    }

    async fn main_loop(&mut self) -> Result<!, Closed> {
        loop {
            if self.tick().has_remaining {
                continue;
            }

            tracing::trace!("worker {} going to sleep...", self.id);
            self.executor.sleepers.wait().await?;
            tracing::trace!("worker woke up");
        }
    }

    pub fn tick(&mut self) -> Tick {
        let mut tick = self.scheduler.tick_n(256);
        tracing::trace!(worker = self.id, ?tick, "worker tick");

        if tick.has_remaining {
            return tick;
        }

        // if there are no tasks remaining in this core's run queue, try to
        // steal new tasks from the distributor queue.
        if let Some(stolen) = self.try_steal() {
            tracing::trace!(tick.stolen = stolen);

            // if we stole tasks, we need to keep ticking
            tick.has_remaining = true;
            return tick;
        }

        // if we have no remaining woken tasks, and we didn't steal any new
        // tasks, this core can sleep until an interrupt occurs.
        tick
    }

    fn try_steal(&mut self) -> Option<NonZeroUsize> {
        const ROUNDS: usize = 4;
        const MAX_STOLEN_PER_TICK: usize = 256;

        // attempt to steal from the global injector queue first
        if let Ok(stealer) = self.executor.injector.try_steal() {
            let stolen = stealer.spawn_n(self.scheduler, MAX_STOLEN_PER_TICK);
            tracing::trace!("stole {stolen} tasks from injector (in first attempt)");
            return NonZeroUsize::new(stolen);
        }

        // If that fails, attempt to steal from other workers
        let num_workers = self.executor.schedulers.len();

        // if there is only one worker, there is no one to steal from anyway
        if num_workers <= 1 {
            return None;
        }

        let mut backoff = Backoff::new();

        for _ in 0..ROUNDS {
            // Start from a random worker
            let start = self.rng.fastrand_n(u32::try_from(num_workers).unwrap()) as usize;

            if let Some(stolen) = self.try_steal_one_round(num_workers, start) {
                return Some(stolen);
            }

            backoff.spin();
        }

        // as a last resort try to steal from the global injector queue again
        if let Ok(stealer) = self.executor.injector.try_steal() {
            let stolen = stealer.spawn_n(self.scheduler, MAX_STOLEN_PER_TICK);
            tracing::trace!("stole {stolen} tasks from injector (in second attempt)");
            return NonZeroUsize::new(stolen);
        }

        None
    }

    fn try_steal_one_round(&mut self, num_workers: usize, start: usize) -> Option<NonZeroUsize> {
        for i in 0..num_workers {
            let i = (start + i) % num_workers;

            // Don't steal from ourselves! We know we don't have work.
            if i == self.id {
                continue;
            }

            let Some(victim) = self.executor.schedulers.iter().nth(i) else {
                // The worker might not be online yet, just advance past
                continue;
            };

            let Ok(stealer) = victim.try_steal() else {
                // the victim either doesn't have any tasks, or is already being stolen from
                // either way, just advance past
                continue;
            };

            let stolen = stealer.spawn_half(self.scheduler);
            tracing::trace!("stole {stolen} tasks from worker {i} {victim:?}");
            return NonZeroUsize::new(stolen);
        }

        None
    }
}

// === impl Scheduler ===

impl Scheduler {
    fn new() -> Result<Self, AllocError> {
        let stub_task = TaskRef::new_stub()?;

        Ok(Self {
            run_queue: MpscQueue::new_with_stub(stub_task),
            queued: AtomicUsize::new(0),
            current_task: AtomicPtr::new(ptr::null_mut()),
            #[cfg(feature = "counters")]
            spawned: AtomicUsize::new(0),
            #[cfg(feature = "counters")]
            woken: AtomicUsize::new(0),
        })
    }

    /// Returns a reference to the task that's current being polled or `None`.
    #[must_use]
    pub fn current_task(&self) -> Option<TaskRef> {
        let ptr = NonNull::new(self.current_task.load(Ordering::Acquire))?;
        Some(TaskRef::clone_from_raw(ptr))
    }

    pub fn schedule(&self, task: TaskRef) {
        self.queued.fetch_add(1, Ordering::AcqRel);
        self.run_queue.enqueue(task);
    }

    fn tick_n(&'static self, n: usize) -> Tick {
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
            tracing::trace!(tick.polled, tick.has_remaining);
            #[cfg(feature = "counters")]
            tracing::trace!(
                tick.polled,
                tick.has_remaining,
                tick.completed,
                tick.spawned,
                tick.woken = tick.woken_external + tick.woken_internal,
                tick.woken.external = tick.woken_external,
                tick.woken.internal = tick.woken_internal
            );
        }

        tick
    }

    fn try_steal(&self) -> Result<Stealer<'_>, TryStealError> {
        Stealer::new(&self.run_queue, &self.queued)
    }
}

#[cfg(test)]
mod tests {
    use core::hint::black_box;
    use core::sync::atomic::AtomicBool;

    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::util::SubscriberInitExt;

    use super::*;
    use crate::{loom, test_util};

    async fn work() -> usize {
        let val = 1 + 1;
        crate::task::yield_now().await;
        black_box(val)
    }

    #[test]
    fn single_threaded() {
        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .set_default();

        loom::model(|| {
            loom::lazy_static! {
                static ref EXEC: Executor = Executor::new().unwrap();
                static ref CALLED: AtomicBool = AtomicBool::new(false);
            }

            EXEC.try_spawn(async move {
                work().await;
                CALLED.store(true, Ordering::SeqCst);
                EXEC.close();
            })
            .unwrap();

            let mut worker = Worker::new(&EXEC, FastRand::from_seed(0)).unwrap();
            test_util::block_on(worker.run(crate::future::pending::<()>())).expect_err(
                "stopping the executor should always result in a Closed(()) error here",
            );
            assert!(CALLED.load(Ordering::SeqCst));
        })
    }

    #[test]
    fn multi_threaded() {
        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .set_default();

        loom::model(|| {
            const NUM_THREADS: usize = 3;

            loom::lazy_static! {
                static ref EXEC: Executor = Executor::new().unwrap();
                static ref CALLED: AtomicBool = AtomicBool::new(false);
            }

            EXEC.try_spawn(async move {
                work().await;
                CALLED.store(true, Ordering::SeqCst);
                EXEC.close();
            })
            .unwrap();

            let joins: Vec<_> = (0..NUM_THREADS)
                .map(|_| {
                    loom::thread::spawn(move || {
                        let mut worker = Worker::new(&EXEC, FastRand::from_seed(0)).unwrap();

                        test_util::block_on(worker.run(crate::future::pending::<()>())).expect_err(
                            "stopping the executor should always result in a Closed(()) error here",
                        );
                    })
                })
                .collect();

            for join in joins {
                join.join().unwrap();
            }
            assert!(CALLED.load(Ordering::SeqCst));
        });
    }

    #[test]
    fn join_handle_cross_thread() {
        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .set_default();

        loom::model(|| {
            loom::lazy_static! {
                static ref EXEC: Executor = Executor::new().unwrap();
            }

            let (tx, rx) = loom::sync::mpsc::channel::<JoinHandle<u32>>();

            let h0 = loom::thread::spawn(move || {
                let tid = loom::thread::current().id();
                let mut worker = Worker::new(&EXEC, FastRand::from_seed(0)).unwrap();

                let h = EXEC
                    .try_spawn(async move {
                        // make sure the task is actually polled on thread 0
                        assert_eq!(loom::thread::current().id(), tid);

                        crate::task::yield_now().await;

                        // make sure the task is actually polled on thread 0
                        assert_eq!(loom::thread::current().id(), tid);

                        42
                    })
                    .unwrap();

                tx.send(h).unwrap();

                test_util::block_on(worker.run(crate::future::pending::<()>())).expect_err(
                    "stopping the executor should always result in a Closed(()) error here",
                );
            });

            let h1 = loom::thread::spawn(move || {
                let h = rx.recv().unwrap();

                let ret_code = test_util::block_on(h).unwrap();

                assert_eq!(ret_code, 42);

                EXEC.close();
            });

            h0.join().unwrap();
            h1.join().unwrap();
        });
    }
}
