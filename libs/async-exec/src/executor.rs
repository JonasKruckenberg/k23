// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::park::{Park, Parker, ParkingLot};
use crate::scheduler::steal::Injector;
use crate::scheduler::{Schedule, Scheduler};
use crate::task::{JoinHandle, TaskBuilder, TaskRef, TaskStub};
use core::alloc::{AllocError, Allocator};
use core::num::NonZeroUsize;
use core::pin::pin;
use core::task::{Context, Poll};
use cpu_local::collection::CpuLocal;
use fastrand::FastRand;
use spin::Backoff;

pub struct Executor<P> {
    schedulers: CpuLocal<Scheduler>,
    stop: AtomicBool,
    parking_lot: ParkingLot<P>,
    injector: Injector<&'static Scheduler>,
    num_stealing: AtomicUsize,
}

pub struct Worker<P: 'static> {
    id: usize,
    exec: &'static Executor<P>,
    scheduler: &'static Scheduler,
    parker: Parker<P>,
    rng: FastRand,
    is_stealing: bool,
}

// === impl Executor ===

impl<P> Executor<P>
where
    P: Park + Send + Sync,
{
    pub fn new(num_workers: usize) -> Self {
        Self {
            schedulers: CpuLocal::with_capacity(num_workers),
            stop: AtomicBool::new(false),
            parking_lot: ParkingLot::new(num_workers),
            injector: Injector::new(),
            num_stealing: AtomicUsize::new(0),
        }
    }

    /// Construct a new `Executor` with a *statically allocated* stub node.
    ///
    /// This constructor is `const` and doesn't require a heap allocation, restrictions on
    /// callers (therefore the `unsafe`). For a safe (but allocating and non-`const`) constructor,
    /// see `[Self::new`].
    ///
    /// # Safety
    ///
    /// The `&'static TaskStub` reference MUST only be used for *this* constructor and **never**
    /// reused for the entire time that `Executor` exists.
    #[cfg(not(loom))]
    #[must_use]
    pub const unsafe fn new_with_static_stub(num_threads: usize, stub: &'static TaskStub) -> Self {
        Self {
            schedulers: CpuLocal::new(),
            stop: AtomicBool::new(false),
            parking_lot: ParkingLot::new(num_threads),
            // Safety: ensured by caller
            injector: unsafe { Injector::new_with_static_stub(stub) },
            num_stealing: AtomicUsize::new(0),
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.parking_lot.unpark_all();
    }

    #[inline]
    pub fn task_builder<'a>(&self) -> TaskBuilder<'a, &'static Scheduler> {
        TaskBuilder::new()
    }

    /// Attempt to spawn this [`Future`] onto the executor.
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output as
    /// well as control some aspects of its runtime behaviour (such as cancelling it).
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    #[inline]
    #[track_caller]
    pub fn try_spawn<F>(&'static self, future: F) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + Send,
        F::Output: Send,
    {
        let (task, join) = self.task_builder().try_build(future)?;
        self.spawn_allocated(task);
        Ok(join)
    }

    /// Attempt to spawn this [`Future`] onto the executor using a custom [`Allocator`].
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output as
    /// well as control some aspects of its runtime behaviour (such as cancelling it).
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
        F: Future + Send,
        F::Output: Send,
        A: Allocator,
    {
        let (task, join) = self.task_builder().try_build_in(future, alloc)?;
        self.spawn_allocated(task);
        Ok(join)
    }

    pub fn spawn_allocated(&'static self, task: TaskRef) {
        if let Some(scheduler) = self.schedulers.get() {
            tracing::trace!("spawning locally {task:?}");
            // we're moving the task to a different scheduler so we need to
            // bind to it
            // Safety: the generics ensure this is always the right type
            unsafe {
                task.bind_scheduler(scheduler);
            }

            scheduler.spawn(task);
        } else {
            tracing::trace!("spawning remote {task:?}");
            self.injector.push_task(task);
            self.parking_lot.unpark_one();
        }
    }

    fn try_transition_worker_to_stealing(&self, worker: &mut Worker<P>) -> bool {
        debug_assert!(!worker.is_stealing);

        let num_stealing = self.num_stealing.load(Ordering::Acquire);
        let num_parked = self.parking_lot.num_parked();

        if 2 * num_stealing >= self.active_workers() - num_parked {
            return false;
        }

        worker.is_stealing = true;
        self.num_stealing.fetch_add(1, Ordering::AcqRel);

        true
    }

    /// A lightweight transition from stealing -> running.
    ///
    /// Returns `true` if this is the final stealing worker. The caller
    /// **must** notify a new worker.
    fn transition_worker_from_stealing(&self, worker: &mut Worker<P>) -> bool {
        debug_assert!(worker.is_stealing);
        worker.is_stealing = false;

        let prev = self.num_stealing.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0);

        prev == 1
    }

    fn active_workers(&self) -> usize {
        self.schedulers.len()
    }

    fn total_workers(&self) -> usize {
        self.parking_lot.capacity()
    }
}

/// Constructs a new [`Executor`] in a safe way.
#[cfg(not(loom))]
#[macro_export]
macro_rules! new_executor {
    ($num_threads:expr) => {{
        static STUB: $crate::task::TaskStub = $crate::task::TaskStub::new();

        // Safety: The intrusive MPSC queue that holds tasks uses a stub node as the initial element of the
        // queue. Being intrusive, the stub can only ever be part of one collection, never multiple.
        // As such, if we were to reuse the stub node it would in effect be unlinked from the previous
        // queue. Which, unlocks a new world of fancy undefined behaviour, but unless you're into that
        // not great.
        // By defining the static above inside this block we guarantee the stub cannot escape
        // and be used elsewhere thereby solving this problem.
        unsafe { $crate::executor::Executor::new_with_static_stub($num_threads, &STUB) }
    }};
}

// === impl Worker ===
impl<P> Worker<P>
where
    P: Park + Send + Sync,
{
    pub fn new(exec: &'static Executor<P>, id: usize, park: P, rng: FastRand) -> Self {
        let scheduler = exec.schedulers.get_or(Scheduler::new);

        Self {
            id,
            exec,
            scheduler,
            parker: Parker::new(park),
            rng,
            is_stealing: false,
        }
    }

    pub fn run(&mut self) {
        let _span = tracing::info_span!("starting worker main loop", worker = self.id).entered();

        loop {
            // drive the scheduling loop until we're out of work
            if self.tick() {
                continue;
            }

            // check the executors signalled us to stop
            if self.exec.stop.load(Ordering::Acquire) {
                tracing::info!(worker = self.id, "stop signal received, shutting down");
                break;
            }

            tracing::debug!("going to sleep");
            // at this point we're fully out of work. We so we should suspend the
            self.exec.parking_lot.park(self.parker.clone());
            tracing::debug!("woke up");
        }
    }

    #[track_caller]
    pub fn block_on<F>(&mut self, future: F) -> F::Output
    where
        F: Future,
    {
        let waker = self.parker.clone().into_unpark().into_waker();
        let mut cx = Context::from_waker(&waker);

        let mut future = pin!(future);

        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }

            // drive the scheduling loop until we're out of work
            if self.tick() {
                continue;
            }

            tracing::debug!("going to sleep");
            // at this point we're fully out of work. We so we should suspend the
            self.exec.parking_lot.park(self.parker.clone());
            tracing::debug!("woke up");
        }
    }

    fn tick(&mut self) -> bool {
        let tick = self.scheduler.tick_n(256);
        tracing::trace!(worker = self.id, ?tick, "worker tick");

        if tick.has_remaining {
            return true;
        }

        if self.exec.try_transition_worker_to_stealing(self) {
            // if there are no tasks remaining in this core's run queue, try to
            // steal new tasks from the distributor queue.
            if let Some(stolen) = self.try_steal() {
                tracing::debug!(tick.stolen = stolen);

                self.exec.transition_worker_from_stealing(self);

                // if we stole tasks, we need to keep ticking
                return true;
            }

            self.exec.transition_worker_from_stealing(self);
            // {
            //     self.exec.parking_lot.unpark_one();
            // }
        }

        // if we have no remaining woken tasks, and we didn't steal any new
        // tasks, this core can sleep until an interrupt occurs.
        false
    }

    fn try_steal(&mut self) -> Option<NonZeroUsize> {
        const ROUNDS: usize = 4;
        const MAX_STOLEN_PER_TICK: NonZeroUsize = NonZeroUsize::new(256).unwrap();

        // attempt to steal from the global injector queue first
        if let Ok(stealer) = self.exec.injector.try_steal() {
            let stolen = stealer.spawn_n(&self.scheduler, MAX_STOLEN_PER_TICK);
            tracing::trace!("stole {stolen} tasks from injector (in first attempt)");
            return Some(stolen);
        }

        // If that fails, attempt to steal from other workers
        let num_workers = self.exec.total_workers();

        // if there is only one worker, there is no one to steal from anyway
        if num_workers <= 1 {
            return None;
        }

        let mut backoff = Backoff::new();

        for _ in 0..ROUNDS {
            // Start from a random worker
            let start = self.rng.fastrand_n(u32::try_from(num_workers).unwrap()) as usize;

            if let Some(stolen) = self.steal_one_round(num_workers, start) {
                return Some(stolen);
            }

            backoff.spin();
        }

        // as a last resort try to steal from the global injector queue again
        if let Ok(stealer) = self.exec.injector.try_steal() {
            let stolen = stealer.spawn_n(&self.scheduler, MAX_STOLEN_PER_TICK);
            tracing::trace!("stole {stolen} tasks from injector (in second attempt)");
            return Some(stolen);
        }

        None
    }

    fn steal_one_round(&mut self, num_workers: usize, start: usize) -> Option<NonZeroUsize> {
        for i in 0..num_workers {
            let i = (start + i) % num_workers;

            // Don't steal from ourselves! We know we don't have work.
            if i == self.id {
                continue;
            }

            let Some(victim) = self.exec.schedulers.iter().nth(i) else {
                // The worker might not be online yet, just advance past
                continue;
            };

            let Ok(stealer) = victim.try_steal() else {
                // the victim either doesn't have any tasks, or is already being stolen from
                // either way, just advance past
                continue;
            };

            let stolen = stealer.spawn_half(&self.scheduler);
            tracing::trace!("stole {stolen} tasks from worker {i} {victim:?}");
            return Some(stolen);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom;
    use crate::park::StdPark;
    use core::hint::black_box;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::util::SubscriberInitExt;

    #[test]
    fn single_threaded_executor() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_thread_names(true)
            .set_default();

        loom::model(|| {
            loom::lazy_static! {
                static ref EXEC: Executor<StdPark> = Executor::new(1);
            }

            let (task, _) = EXEC
                .task_builder()
                .try_build(async move {
                    tracing::info!("Hello World!");
                    EXEC.stop();
                })
                .unwrap();
            EXEC.spawn_allocated(task);

            let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

            worker.run();
        })
    }

    #[test]
    fn multi_threaded_executor() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_thread_names(true)
            .set_default();

        loom::model(|| {
            const NUM_THREADS: usize = 3;

            loom::lazy_static! {
                static ref EXEC: Executor<StdPark> = Executor::new(NUM_THREADS);
            }

            let (task, _) = EXEC
                .task_builder()
                .try_build(async move {
                    tracing::info!("Hello World!");
                    EXEC.stop();
                })
                .unwrap();
            EXEC.spawn_allocated(task);

            let joins: Vec<_> = (0..NUM_THREADS)
                .map(|id| {
                    loom::thread::Builder::new()
                        .name(format!("Worker(0{id})"))
                        .spawn(move || {
                            let mut worker = Worker::new(
                                &EXEC,
                                id,
                                StdPark::for_current(),
                                FastRand::from_seed(0),
                            );

                            worker.run();
                        })
                        .unwrap()
                })
                .collect();

            for join in joins {
                join.join().unwrap();
            }
        })
    }

    #[test]
    fn block_on() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .try_init();

        async fn work(num_polls: &AtomicUsize) -> usize {
            num_polls.fetch_add(1, Ordering::Relaxed);

            let val = 1 + 1;
            crate::task::yield_now().await;
            num_polls.fetch_add(1, Ordering::Relaxed);

            black_box(val)
        }

        loom::model(|| {
            loom::lazy_static! {
                static ref NUM_POLLS: AtomicUsize = AtomicUsize::new(0);
                static ref EXEC: Executor<StdPark> = Executor::new(1);
            }

            let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

            worker.block_on(async {
                let (task, h) = EXEC.task_builder().try_build(work(&NUM_POLLS)).unwrap();
                EXEC.spawn_allocated(task);
                assert_eq!(h.await.unwrap(), 2);
            });

            assert_eq!(NUM_POLLS.load(Ordering::Relaxed), 2);
        })
    }
}
