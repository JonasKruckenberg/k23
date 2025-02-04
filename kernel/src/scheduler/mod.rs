// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod idle;
mod queue;
mod worker;
mod yield_now;

use crate::arch::device::cpu::with_cpu_info;
use crate::hart_local::HartLocal;
use crate::scheduler::idle::Idle;
use crate::task;
use crate::task::{JoinHandle, OwnedTasks, TaskRef};
use crate::time::Timer;
use crate::util::condvar::Condvar;
use crate::util::fast_rand::FastRand;
use crate::util::parking_spot::ParkingSpot;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::future::Future;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Waker;
use rand::RngCore;
use sync::{Mutex, OnceLock};

static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();

/// Get a reference to the current executor.
pub fn current() -> &'static Scheduler {
    SCHEDULER.get().expect("scheduler not initialized")
}

/// Initialize the global executor.
///
/// This will allocate required state for `num_cores` of harts. Tasks can immediately be spawned
/// using the returned runtime reference (a reference to the runtime can also be obtained using
/// [`current()`]) but no tasks will run until at least one hart in the system enters its
/// runtime loop using [`run()`].
#[cold]
pub fn init(num_cores: u32, rng: &mut impl RngCore, shutdown_on_idle: bool) -> &'static Scheduler {
    SCHEDULER.get_or_init(|| Scheduler::new(num_cores as usize, rng, shutdown_on_idle))
}

/// Run the async runtime loop on the calling hart.
///
/// This function will not return until the runtime is shut down.
#[inline]
pub fn run(sched: &'static Scheduler, hartid: usize, initial: impl FnOnce()) -> Result<(), ()> {
    let clock = with_cpu_info(|info| info.clock.clone());
    let timer = Timer::new(clock);
    worker::run(sched, timer, hartid, initial)
}

pub struct Scheduler {
    shared: worker::Shared,
}

impl Scheduler {
    #[expect(tail_expr_drop_order, reason = "")]
    pub fn new(num_cores: usize, rand: &mut impl RngCore, shutdown_on_idle: bool) -> Self {
        let mut cores = Vec::with_capacity(num_cores);
        let mut remotes = Vec::with_capacity(num_cores);

        for i in 0..num_cores {
            let (steal, run_queue) = queue::new();

            cores.push(Box::new(worker::Core {
                index: i,
                run_queue,
                lifo_slot: None,
                is_searching: false,
                rand: FastRand::new(rand.next_u64()),
            }));
            remotes.push(worker::Remote { steal });
        }

        let (idle, idle_synced) = Idle::new(cores);

        let stub = TaskRef::new_stub();
        let run_queue = mpsc_queue::MpscQueue::new_with_stub(stub);

        Self {
            shared: worker::Shared {
                shutdown: AtomicBool::new(false),
                remotes: remotes.into_boxed_slice(),
                owned: OwnedTasks::new(),
                synced: Mutex::new(worker::Synced {
                    assigned_cores: (0..num_cores).map(|_| None).collect(),
                    idle: idle_synced,
                    shutdown_cores: Vec::with_capacity(num_cores),
                }),
                run_queue,
                idle,
                condvars: (0..num_cores).map(|_| Condvar::new()).collect(),
                parking_spot: ParkingSpot::default(),
                per_hart: HartLocal::with_capacity(num_cores),
                shutdown_on_idle,
            },
        }
    }

    pub fn spawn<F>(&'static self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let id = task::Id::next();
        let (handle, maybe_task) = self.shared.owned.bind(future, self, id);

        if let Some(task) = maybe_task {
            self.shared.schedule_task(task, false);
        }

        handle
    }

    pub fn shutdown(&self) {
        if !self.shared.shutdown.swap(true, Ordering::AcqRel) {
            let mut synced = self.shared.synced.lock();

            // Set the shutdown flag on all available cores
            self.shared.idle.shutdown(&mut synced, &self.shared);

            // Any unassigned cores need to be shutdown, but we have to first drop
            // the lock
            drop(synced);
            self.shared.idle.shutdown_unassigned_cores(&self.shared);
        }
    }

    #[inline]
    pub(crate) fn defer(&self, waker: &Waker) {
        self.shared.per_hart.get().unwrap().defer(waker);
    }

    #[inline]
    pub(crate) fn timer(&self) -> &Timer {
        self.shared.per_hart.get().unwrap().timer()
    }
}

impl task::Schedule for &'static Scheduler {
    fn schedule(&self, task: TaskRef) {
        self.shared.schedule_task(task, false);
    }

    fn release(&self, task: &TaskRef) -> Option<TaskRef> {
        self.shared.owned.remove(task)
    }

    fn yield_now(&self, task: TaskRef) {
        self.shared.schedule_task(task, true);
    }
}
