// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::park::{Park, Parker, ParkingLot};
use crate::scheduler::{Injector, Scheduler};
use crate::task::{JoinHandle, TaskBuilder, TaskStub};
use core::alloc::{AllocError, Allocator};
use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicBool, Ordering};
use cpu_local::collection::CpuLocal;
use fastrand::FastRand;
use spin::Backoff;
use util::loom_const_fn;

pub struct Executor<P> {
    schedulers: CpuLocal<Scheduler>,
    injector: Injector<Scheduler>,
    parking_lot: ParkingLot<P>,
    is_running: AtomicBool,
    // shutdown_barrier: Barrier,
}

pub struct Worker<P: 'static> {
    executor: &'static Executor<P>,
    scheduler: &'static Scheduler,
    id: usize,
    rng: FastRand,
    parker: Parker<P>,
    _m: PhantomData<*mut u8>,
}

impl<P: Park + Send + Sync + 'static> Executor<P> {
    pub fn new(num_threads: usize) -> Self {
        Self {
            schedulers: CpuLocal::with_capacity(num_threads),
            injector: Injector::new(),
            parking_lot: ParkingLot::new(num_threads),
            is_running: AtomicBool::new(true),
            // shutdown_barrier: Barrier::new(num_threads),
        }
    }

    loom_const_fn! {
        pub const unsafe fn new_with_static_stub(stub: &'static TaskStub, num_threads: usize) -> Self {
            Self {
                schedulers: CpuLocal::new(),
                // Safety: ensured by caller
                injector: unsafe { Injector::new_with_static_stub(stub) },
                parking_lot: ParkingLot::new(num_threads),
                is_running: AtomicBool::new(true),
                // shutdown_barrier: Barrier::new(num_threads),
            }
        }
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::Release);
        // tracing::debug!("Executor::stop: before unparking...");
        let unparked = self.parking_lot.unpark_all();
        // tracing::debug!("Executor::stop: unparked {unparked} workers for shutdown");
    }

    #[inline]
    pub fn local_scheduler(&self) -> Option<&Scheduler> {
        self.schedulers.get()
    }

    /// Returns a new [`TaskBuilder`] for configuring tasks prior to spawning them
    /// onto this scheduler.
    #[must_use]
    #[inline]
    pub fn build_task(&self) -> TaskBuilder<Scheduler> {
        if let Some(scheduler) = self.schedulers.get() {
            scheduler.build_task()
        } else {
            self.injector.build_task()
        }
    }

    /// Attempt to spawn a given [`Future`] onto this scheduler.
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output
    /// as well as control some aspects of its runtime behaviour (such as cancelling it).
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
}

// #[macro_export]
// macro_rules! new_executor {
//     ($num_threads:expr) => {{
//         static STUB: $crate::task::TaskStub = $crate::task::TaskStub::new();
//
//         // Safety: The intrusive MPSC queue that holds tasks uses a stub node as the initial element of the
//         // queue. Being intrusive, the stub can only ever be part of one collection, never multiple.
//         // As such, if we were to reuse the stub node it would in effect be unlinked from the previous
//         // queue. Which, unlocks a new world of fancy undefined behaviour, but unless you're into that
//         // not great.
//         // By defining the static above inside this block we guarantee the stub cannot escape
//         // and be used elsewhere thereby solving this problem.
//         unsafe { $crate::executor::Executor::new_with_static_stub(&STUB, $num_threads) }
//     }};
// }

// === impl Worker ===

impl<P: Park + Send + Sync> Worker<P> {
    pub fn new(executor: &'static Executor<P>, id: usize, rng: FastRand, park: P) -> Self {
        let scheduler = executor.schedulers.get_or(|| Scheduler::new());

        Self {
            executor,
            scheduler,
            id,
            rng,
            parker: Parker::new(park),
            _m: PhantomData,
        }
    }

    pub fn is_running(&self) -> bool {
        self.executor.is_running.load(Ordering::Acquire)
    }

    pub fn run(&mut self) {
        let _span = tracing::info_span!("starting worker main loop", worker = self.id).entered();

        loop {
            // tick the scheduler until it indicates that it's out of tasks to run.
            if self.tick() {
                continue;
            }

            // check if this core should shut down.
            if !self.is_running() {
                tracing::info!(worker = self.id, "stop signal received, shutting down");
                break;
            }

            // at this point there is no more work for us to do
            // lets just park
            self.executor.parking_lot.park(self.parker.clone());
        }

        assert!(!self.is_running());

        // tracing::debug!("waiting for other workers to terminate");
        // self.executor.shutdown_barrier.wait();
    }

    fn tick(&mut self) -> bool {
        // drive the task scheduler
        let tick = self.scheduler.tick();

        // if there are remaining tasks to poll, continue without stealing.
        if tick.has_remaining {
            return true;
        }

        // if there are no tasks remaining in this core's run queue, try to
        // steal new tasks from the distributor queue.
        if let Some(stolen) = self.try_steal() {
            tracing::debug!(tick.stolen = stolen);
            // if we stole tasks, we need to keep ticking
            return true;
        }

        // if we have no remaining woken tasks, and we didn't steal any new
        // tasks, this core can sleep until an interrupt occurs.
        false
    }

    fn try_steal(&mut self) -> Option<NonZeroUsize> {
        const ROUNDS: usize = 4;
        const MAX_STOLEN_PER_TICK: NonZeroUsize = NonZeroUsize::new(256).unwrap();

        // attempt to steal from the global injector queue first
        if let Ok(stealer) = self.executor.injector.try_steal() {
            let stolen = stealer.spawn_n(&self.scheduler, MAX_STOLEN_PER_TICK);
            return Some(stolen);
        }

        // If that fails, attempt to steal from other workers
        let num_workers = self.executor.schedulers.len();

        // if there is only one worker, there is no one to steal from anyways
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
        if let Ok(stealer) = self.executor.injector.try_steal() {
            let stolen = stealer.spawn_n(&self.scheduler, MAX_STOLEN_PER_TICK);
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

            let Some(victim) = self.executor.schedulers.iter().nth(i) else {
                // The worker might not be online yet, just advance past
                continue;
            };

            let Ok(stealer) = victim.try_steal() else {
                // the victim either doesn't have any tasks, or is already being stolen from
                // either way, just advance past
                continue;
            };

            return Some(stealer.spawn_half(&self.scheduler));
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom;
    use crate::park::StdPark;
    use alloc::vec::Vec;
    use tracing_subscriber::EnvFilter;

    #[test]
    fn executor_multi_thread() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .try_init();

        loom::model(|| {
            const NUM_THREADS: usize = 3;

            loom::lazy_static! {
                static ref EXEC: Executor<StdPark> = Executor::new(NUM_THREADS);
            }

            EXEC.try_spawn(async {
                tracing::info!("hello world");
                EXEC.stop();
            })
            .unwrap();

            let joins: Vec<_> = (0..NUM_THREADS)
                .map(|id| {
                    loom::thread::spawn(move || {
                        let mut worker =
                            Worker::new(&EXEC, id, FastRand::from_seed(0), StdPark::for_current());

                        worker.run();
                    })
                })
                .collect();

            for join in joins {
                join.join().unwrap();
            }
        })
    }
}
