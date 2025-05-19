// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::cpu_local::CpuLocal;
use crate::time::global_timer;
use crate::{CPUID, arch};
use async_kit::new_static_scheduler;
use async_kit::park::{Park, Parker, ParkingLot};
use async_kit::scheduler::{Injector, StaticScheduler};
use async_kit::task::TaskStub;
use async_kit::task::{JoinHandle, TaskBuilder};
use core::alloc::{AllocError, Allocator};
use core::num::NonZeroUsize;
use core::pin::pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll};
use cpu_local::cpu_local;
use fastrand::FastRand;
use rand::Rng;
use spin::{Backoff, Barrier, OnceLock};

static SCHEDULER: OnceLock<Runtime> = OnceLock::new();

pub fn init(num_cores: usize) -> &'static Runtime {
    static TASK_STUB: TaskStub = TaskStub::new();

    SCHEDULER.get_or_init(|| Runtime {
        workers: CpuLocal::new(),
        // Safety: ensured by caller
        injector: unsafe { Injector::new_with_static_stub(&TASK_STUB) },
        shutdown: AtomicBool::new(false),
        shutdown_barrier: Barrier::new(num_cores),
        parking_lot: ParkingLot::new(num_cores),
    })
}

pub fn runtime() -> &'static Runtime {
    SCHEDULER.get().expect("scheduler not initialized")
}

#[inline]
#[track_caller]
pub fn try_spawn<F>(future: F) -> Result<JoinHandle<F::Output>, AllocError>
where
    F: Future + Send + 'static,
{
    SCHEDULER
        .get()
        .expect("scheduler not initialized")
        .try_spawn(future)
}

pub struct Runtime {
    workers: CpuLocal<StaticScheduler>,
    injector: Injector<&'static StaticScheduler>,
    shutdown: AtomicBool,
    /// Spin barrier used to synchronize shutdown between workers,
    /// see comments in [`Worker::stop`] for details.
    shutdown_barrier: Barrier,
    parking_lot: ParkingLot<InterruptPark>,
}

pub struct Worker {
    runtime: &'static Runtime,
    scheduler: &'static StaticScheduler,
    id: usize,
    rng: FastRand,
    is_running: AtomicBool,
}

impl Runtime {
    #[inline]
    pub fn local_scheduler(&'static self) -> Option<&'static StaticScheduler> {
        self.workers.get()
    }

    /// Returns a new [`TaskBuilder`] for configuring tasks prior to spawning them
    /// onto this scheduler.
    #[must_use]
    #[inline]
    pub fn build_task<'a>(&'static self) -> TaskBuilder<'a, &'static StaticScheduler> {
        if let Some(scheduler) = self.workers.get() {
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

    #[track_caller]
    pub fn block_on<F>(&'static self, future: F) -> F::Output
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        cpu_local! {
            static PARKER: Parker<InterruptPark> = Parker::new(InterruptPark { cpuid: CPUID.get() });
        }
        let waker = PARKER.with(|parker| parker.clone().into_waker());

        let mut cx = Context::from_waker(&waker);
        let mut future = pin!(future);

        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }

            PARKER.with(|parker| parker.park());
        }
    }
}

impl Worker {
    pub fn new(runtime: &'static Runtime, id: usize, rng: &mut impl Rng) -> Worker {
        let scheduler = runtime.workers.get_or(|| new_static_scheduler!());

        Worker {
            runtime,
            scheduler,
            id,
            rng: FastRand::from_seed(rng.next_u64()),
            is_running: AtomicBool::new(false),
        }
    }

    pub fn stop(&self) -> bool {
        let was_running = self
            .is_running
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        tracing::info!(core = self.id, core.was_running = was_running, "stopping");
        was_running
    }

    pub fn run(&mut self) {
        let _span = tracing::info_span!("core", id = self.id).entered();
        if self
            .is_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            tracing::error!("this worker is already running!");
            return;
        }

        tracing::info!("started worker main loop");

        loop {
            // tick the scheduler until it indicates that it's out of tasks to run.
            if self.tick() {
                continue;
            }

            // check if this core should shut down.
            if !self.is_running() {
                tracing::info!(core = self.id, "stop signal received, shutting down");
                break;
            }

            let parker = Parker::new(InterruptPark { cpuid: self.id });
            self.runtime.parking_lot.park(parker);
        }

        assert!(
            self.is_running
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        );

        self.runtime.shutdown_barrier.wait();
    }

    fn tick(&mut self) -> bool {
        // drive the task scheduler
        let tick = self.scheduler.tick();

        // turn the timer wheel if it wasn't turned recently and no one else is
        // holding a lock, ensuring any pending timer ticks are consumed.
        global_timer().turn();

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
        if let Ok(stealer) = self.runtime.injector.try_steal() {
            let stolen = stealer.spawn_n(&self.scheduler, MAX_STOLEN_PER_TICK);
            return Some(stolen);
        }

        // If that fails, attempt to steal from other workers
        let num_workers = self.runtime.workers.len();

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
        if let Ok(stealer) = self.runtime.injector.try_steal() {
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

            let Some(victim) = self.runtime.workers.iter().nth(i) else {
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

    fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Acquire)
    }
}

struct InterruptPark {
    cpuid: usize,
}

// Safety: TODO
impl Park for InterruptPark {
    fn park(&self) {
        // Safety: TODO
        unsafe { arch::cpu_park() }
    }

    fn unpark(&self) {
        // Safety: TODO
        unsafe { arch::cpu_unpark(self.cpuid) }
    }
}
