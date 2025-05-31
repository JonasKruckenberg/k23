// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::park::{Park, Parker, ParkingLot};
use crate::scheduler::steal::{Injector, Stealer, TryStealError};
use crate::scheduler::{Schedule, Scheduler, Tick};
use crate::task::{JoinHandle, TaskBuilder, TaskRef, TaskStub};
use crate::time::{Clock, Timer};
use core::alloc::{AllocError, Allocator};
use core::num::NonZeroUsize;
use core::pin::pin;
use core::task::{Context, Poll};
use cpu_local::collection::CpuLocal;
use fastrand::FastRand;
use spin::Backoff;

#[derive(Debug)]
pub struct Executor<P> {
    schedulers: CpuLocal<Scheduler>,
    stop: AtomicBool,
    parking_lot: ParkingLot<P>,
    injector: Injector<&'static Scheduler>,
    num_stealing: AtomicUsize,
    timer: Timer,
}

#[derive(Debug)]
pub struct Worker<P: 'static> {
    id: usize,
    exec: &'static Executor<P>,
    scheduler: &'static Scheduler,
    parker: Parker<P>,
    rng: FastRand,
    is_stealing: bool,
}

impl<P> Executor<P> {
    #[inline]
    pub fn timer(&self) -> &Timer {
        &self.timer
    }
}

// === impl Executor ===

impl<P> Schedule for &'static Executor<P> {
    fn current_task(&self) -> Option<TaskRef> {
        self.schedulers.get()?.current_task()
    }

    fn spawn(&self, task: TaskRef) {
        if let Some(scheduler) = self.schedulers.get() {
            scheduler.spawn(task);
        } else {
            self.injector.push_task(task);
        }
    }

    fn wake(&self, task: TaskRef) {
        if let Some(scheduler) = self.schedulers.get() {
            scheduler.wake(task);
        } else {
            self.injector.push_task(task);
        }
    }
}

impl<P> Executor<P>
where
    P: Park + Send + Sync,
{
    pub fn new(num_workers: usize, clock: Clock) -> Self {
        Self {
            schedulers: CpuLocal::with_capacity(num_workers),
            stop: AtomicBool::new(false),
            parking_lot: ParkingLot::with_capacity(num_workers),
            injector: Injector::new(),
            num_stealing: AtomicUsize::new(0),
            timer: Timer::new(clock),
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
    pub const unsafe fn new_with_static_stub(clock: Clock, stub: &'static TaskStub) -> Self {
        Self {
            schedulers: CpuLocal::new(),
            stop: AtomicBool::new(false),
            parking_lot: ParkingLot::new(),
            // Safety: ensured by caller
            injector: unsafe { Injector::new_with_static_stub(stub) },
            num_stealing: AtomicUsize::new(0),
            timer: Timer::new(clock),
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.parking_lot.unpark_all();
    }

    /// Returns the CPU local scheduler
    ///
    /// # Panics
    ///
    /// Panics if there is no active scheduler on this CPU (the worker hasn't started yet)
    pub fn cpu_local_scheduler(&self) -> &Scheduler {
        self.schedulers.get().expect("no active scheduler")
    }

    #[inline]
    pub fn task_builder<'a>(&self) -> TaskBuilder<'a, &'static Scheduler, ()> {
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
        let _span = tracing::debug_span!("worker main loop", worker = self.id).entered();

        loop {
            // drive the scheduling loop until we're out of work
            if self.tick() {
                continue;
            }

            // check the executors signalled us to stop
            if self.exec.stop.load(Ordering::Acquire) {
                tracing::debug!(worker = self.id, "stop signal received, shutting down");
                break;
            }

            tracing::trace!("turning timer...");
            let (expired, maybe_next_deadline) = self.exec.timer.try_turn().unwrap_or((0, None));

            // if turning the timer expired some `Sleep`s that means we potentially unblocked
            // some tasks. let's try polling again!
            if expired > 0 {
                continue;
            }

            tracing::trace!(maybe_next_deadline = ?maybe_next_deadline, "going to sleep");
            if let Some(next_deadline) = maybe_next_deadline {
                self.parker
                    .park_until(next_deadline, self.exec.timer.clock());
            } else {
                self.parker.park();
            }
            tracing::trace!("woke up");
        }
    }

    #[track_caller]
    pub fn block_on<F>(&mut self, future: F) -> F::Output
    where
        F: Future,
    {
        let _span = tracing::debug_span!("worker block_on", worker = self.id).entered();

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

            tracing::trace!("turning timer...");
            let (expired, maybe_next_deadline) = self.exec.timer.try_turn().unwrap_or((0, None));

            // if turning the timer expired some `Sleep`s that means we potentially unblocked
            // some tasks. let's try polling again!
            if expired > 0 {
                continue;
            }

            tracing::trace!(maybe_next_deadline = ?maybe_next_deadline, "going to sleep");
            if let Some(next_deadline) = maybe_next_deadline {
                self.parker
                    .park_until(next_deadline, self.exec.timer.clock());
            } else {
                self.parker.park();
            }
            tracing::trace!("woke up");
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
                tracing::trace!(tick.stolen = stolen);

                self.exec.transition_worker_from_stealing(self);

                // if we stole tasks, we need to keep ticking
                return true;
            }

            self.exec.transition_worker_from_stealing(self);
        }

        // if we have no remaining woken tasks, and we didn't steal any new
        // tasks, this core can sleep until an interrupt occurs.
        false
    }

    fn try_steal(&mut self) -> Option<NonZeroUsize> {
        const ROUNDS: usize = 4;
        const MAX_STOLEN_PER_TICK: usize = 256;

        // attempt to steal from the global injector queue first
        if let Ok(stealer) = self.exec.injector.try_steal() {
            let stolen = stealer.spawn_n(self.scheduler, MAX_STOLEN_PER_TICK);
            tracing::trace!("stole {stolen} tasks from injector (in first attempt)");
            return NonZeroUsize::new(stolen);
        }

        // If that fails, attempt to steal from other workers
        let num_workers = self.exec.active_workers();

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
            return NonZeroUsize::new(stolen);
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
            return NonZeroUsize::new(stolen);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom;
    use crate::task::{Id, Task};
    use crate::test_util::StopOnPanic;
    use crate::test_util::{StdPark, std_clock};
    use alloc::boxed::Box;
    use alloc::sync::Arc;
    use core::any::type_name;
    use core::hint::black_box;
    use core::marker::PhantomData;
    use core::panic::Location;
    use core::pin::Pin;
    use spin::RwLock;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::util::SubscriberInitExt;

    #[test]
    fn single_threaded_executor() {
        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_thread_names(true)
            .set_default();

        loom::model(|| {
            loom::lazy_static! {
                static ref EXEC: Executor<StdPark> = Executor::new(1, std_clock!());
            }

            EXEC.try_spawn(async move {
                tracing::info!("Hello World!");
                EXEC.stop();
            })
            .unwrap();

            let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

            worker.run();
        })
    }

    // FIXME loom doesn't like this test... It would be great to figure out exactly why
    //  and fix that, you know for like, correctness.
    #[cfg(not(loom))]
    #[test]
    fn multi_threaded_executor() {
        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_thread_names(true)
            .set_default();

        loom::model(|| {
            const NUM_THREADS: usize = 3;

            loom::lazy_static! {
                static ref EXEC: Executor<StdPark> = Executor::new(NUM_THREADS, std_clock!());
            }

            EXEC.try_spawn(async {
                tracing::info!("Hello World!");
                EXEC.stop();
            })
            .unwrap();

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
        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_thread_ids(true)
            .set_default();

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
                static ref EXEC: Executor<StdPark> = Executor::new(1, std_clock!());
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

    #[test]
    fn join_handle_cross_thread() {
        loom::model(|| {
            loom::lazy_static! {
                static ref EXEC: Executor<StdPark> = Executor::new(2, std_clock!());
            }

            let _guard = StopOnPanic::new(&EXEC);

            let (tx, rx) = loom::sync::mpsc::channel::<JoinHandle<u32>>();

            let h0 = loom::thread::spawn(move || {
                let tid = loom::thread::current().id();

                let mut worker =
                    Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));

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

                worker.run();
            });
            let h1 = loom::thread::spawn(move || {
                let mut worker =
                    Worker::new(&EXEC, 1, StdPark::for_current(), FastRand::from_seed(0));

                let h = rx.recv().unwrap();

                let ret_code = worker.block_on(h).unwrap();

                assert_eq!(ret_code, 42);

                EXEC.stop();
            });

            h0.join().unwrap();
            h1.join().unwrap();
        });
    }

    #[test]
    fn miri_check() {
        let (tx, rx) = loom::sync::mpsc::channel::<u32>();

        let h0 = loom::thread::spawn(move || {
            tx.send(42).unwrap();
        });
        let h1 = loom::thread::spawn(move || {
            assert_eq!(rx.recv().unwrap(), 42);
        });

        h0.join().unwrap();
        h1.join().unwrap();
    }

    #[test]
    fn builder() {
        loom::lazy_static! {
            static ref EXEC: Executor<StdPark> = Executor::new(1, std_clock!());
        }

        impl<P> Executor<P> {
            pub fn __task_builder<'a>(&'static self) -> TaskBuilder<'a, &'static Self, ()> {
                TaskBuilder::new(self)
            }
        }

        struct TaskBuilder<'a, S, E> {
            location: Option<Location<'a>>,
            name: Option<&'a str>,
            kind: &'a str,
            ext: E,
            scheduler: S,
        }

        impl<'a, S> TaskBuilder<'a, S, ()> {
            pub const fn new(scheduler: S) -> Self {
                Self {
                    location: None,
                    name: None,
                    kind: "",
                    ext: (),
                    scheduler,
                }
            }
        }

        impl<'a, S, E> TaskBuilder<'a, S, E> {
            /// Override the name of tasks spawned by this builder.
            ///
            /// By default, tasks are unnamed.
            pub fn name(mut self, name: &'a str) -> Self {
                self.name = Some(name);
                self
            }

            /// Override the kind string of tasks spawned by this builder, this will only show up
            /// in debug messages and spans.
            ///
            /// By default, tasks are of kind `"kind"`.
            pub fn kind(mut self, kind: &'a str) -> Self {
                self.kind = kind;
                self
            }

            /// Override the source code location that will be associated with tasks spawned by this builder.
            ///
            /// By default, tasks will inherit the source code location of where they have been first spawned.
            pub fn location(mut self, location: Location<'a>) -> Self {
                self.location = Some(location);
                self
            }

            pub fn ext<N>(mut self, ext: N) -> TaskBuilder<'a, S, N> {
                TaskBuilder {
                    location: self.location,
                    name: self.name,
                    kind: self.kind,
                    ext,
                    scheduler: self.scheduler,
                }
            }

            #[inline]
            #[track_caller]
            pub fn try_spawn<F>(
                self,
                future: F,
            ) -> Result<(TaskRef, JoinHandle<F::Output>), AllocError>
            where
                F: Future + Send,
                F::Output: Send,
                S: Schedule,
            {
                self.try_spawn_in(future, alloc::alloc::Global)
            }

            #[inline]
            #[track_caller]
            pub fn try_spawn_in<F, A>(
                self,
                future: F,
                alloc: A,
            ) -> Result<(TaskRef, JoinHandle<F::Output>), AllocError>
            where
                F: Future + Send,
                F::Output: Send,
                S: Schedule,
                A: Allocator,
            {
                let id = Id::next();

                let loc = self.location.as_ref().unwrap_or(Location::caller());
                let span = tracing::trace_span!(
                    "task",
                    task.tid = id.as_u64(),
                    task.name = ?self.name,
                    task.kind = self.kind,
                    task.output = %type_name::<F::Output>(),
                    loc.file = loc.file(),
                    loc.line = loc.line(),
                    loc.col = loc.column(),
                );

                let task = Task::<F, S, E>::new(future, id, self.ext, span);
                let task = Box::try_new_in(task, alloc)?;

                Ok(TaskRef::new_allocated(task))
            }
        }

        EXEC.__task_builder()
            .name("wasm task")
            .kind("wasm task")
            .ext("foo")
            .try_spawn(async {});
    }

    // #[test]
    // fn context_test() {
    //     // === addr space ===
    //
    //     struct AddressSpace;
    //     impl AddressSpace {
    //         pub fn page_fault(&mut self) {
    //             println!("page fault");
    //         }
    //     }
    //
    //     // === fut ===
    //
    //     struct Fut {}
    //     impl Future for Fut {
    //         type Output = ();
    //
    //         fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    //             let aspace = cx
    //                 .ext()
    //                 .downcast_mut::<Arc<RwLock<AddressSpace>>>()
    //                 .unwrap();
    //             if let Some(mut aspace) = aspace.try_write() {
    //                 aspace.page_fault();
    //                 Poll::Ready(())
    //             } else {
    //                 todo!("should return pending here")
    //             }
    //         }
    //     }
    //
    //     // === exec ===
    //
    //     loom::lazy_static! {
    //         static ref EXEC: Executor<StdPark> = Executor::new(1, std_clock!());
    //     }
    //
    //     let aspace = Arc::new(RwLock::new(AddressSpace {}));
    //
    //     let (task, h) = EXEC
    //         .task_builder()
    //         .ext(aspace.clone())
    //         .try_build(Fut {})
    //         .unwrap();
    //     EXEC.spawn_allocated(task);
    //
    //     let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));
    //
    //     worker.block_on(h).unwrap();
    // }
}
