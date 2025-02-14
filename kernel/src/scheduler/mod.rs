// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! # Scheduler Subsystem
//!
//! The scheduler subsystem is responsible for managing the execution of tasks on the system. Contrary
//! to traditional operating systems, tasks in k23 are lightweight stackless coroutines that are scheduled
//! cooperatively (i.e. Rust futures). The scheduler is responsible for managing the lifecycle of these
//! as well as scheduling them across multiple cores.
//!
//! The current implementation is adapted from Eliza Weisman's great [`maitake`] crate and [`tokio`]s
//! [`MultiThreadAlt`] scheduler implementation.
//!
//! From a super high level the scheduler is a collection of [`Task`][TaskRef]s that need to be scheduled.
//! A task essentially being a [`Future`] and associated lifecycle metadata.
//! The scheduler will repeatedly remove a task from that collection and schedule it (by calling
//! the futures [`poll`][Future::poll] method). When the collection is empty,the CPU will go to sleep
//! until a task is added to the collection.
//!
//! ## Synchronization primitives
//!
//! TODO
//!
//! ## Interrupts
//!
//! TODO
//!
//! ## Timers
//!
//! The scheduler provides a specialized system for tracking time that under the hood uses timer interrupts,
//! but presents them in a more usable fashion.
//!
//! The timekeeping primitives can be found [here][crate::time].
//!
//! Each [`Worker`] has their own local timer. When the [`Worker`] has exhausted all tasks in their
//! local queue, right before going to sleep or after having scheduled [`DEFAULT_GLOBAL_QUEUE_INTERVAL`]
//! tasks in a row, the worker will [`turn`][Timer::turn] their timer. This will cause two thing:
//!
//! 1. The hardware timer will be reset to fire at the new deadline
//! 2. Any tasks than have registered with the timer will be woken (through their [`Waker`][core::task::Waker]).
//!
//! ## Detailed description of current behaviour
//!
//! The scheduler has a fixed number of [`Worker`]s one for each CPU of the system. Each `Worker`
//! maintains their own fixed-size local queue of tasks ([`queue::LOCAL_QUEUE_CAPACITY`] elements).
//! If more tasks are added to the local run queue than can fit, then half of them are moved to the
//! global queue to make space.
//!
//! Workers will prefer tasks from their own local queue and will only pick a task from the global queue
//! if the local queue is empty, or if it has picked a task from the local queue `global_queue_interval`
//! times in a row (currently non-configurable and fixed at [`DEFAULT_GLOBAL_QUEUE_INTERVAL`]).
//!
//! If both the local queue and global queue is empty, then the worker thread will attempt to steal tasks
//! from the local queue of another worker thread. Stealing is done by moving half of the tasks in one
//! local queue to another local queue.
//!
//! [`maitake`]: https://github.com/hawkw/mycelium/tree/dd0020892564c77ee4c20ffbc2f7f5b046ad54c8/maitake
//! [`tokio`]: https://tokio.rs
//! [`MultiThreadAlt`]: https://docs.rs/tokio/latest/tokio/runtime/struct.Builder.html#method.new_multi_thread_alt

mod idle;
mod queue;
mod yield_now;

use crate::cpu_local::CpuLocal;
use crate::scheduler::idle::Idle;
use crate::scheduler::queue::Overflow;
use crate::task::{JoinHandle, OwnedTasks, PollResult, Schedule, TaskRef};
use crate::time::Timer;
use crate::util::fast_rand::FastRand;
use crate::{arch, task};
use core::cell::RefCell;
use core::future::Future;
use core::mem;
use core::ops::DerefMut;
use core::sync::atomic::{AtomicBool, Ordering};
use rand::RngCore;
use sync::{Backoff, Barrier, OnceLock};

const DEFAULT_GLOBAL_QUEUE_INTERVAL: u32 = 61;

static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();

pub fn init(num_cores: usize) -> &'static Scheduler {
    static TASK_STUB: TaskStub = TaskStub::new();

    #[expect(tail_expr_drop_order, reason = "")]
    SCHEDULER.get_or_init(|| Scheduler {
        cores: CpuLocal::with_capacity(num_cores),
        remotes: CpuLocal::with_capacity(num_cores),
        owned: OwnedTasks::new(),
        // Safety: the static is scoped to this function AND we call `new_with_static_stub` through
        // `get_or_init` which guarantees the stub will only ever be used once here.
        run_queue: unsafe { mpsc_queue::MpscQueue::new_with_static_stub(&TASK_STUB.hdr) },
        shutdown: AtomicBool::new(false),
        idle: Idle::new(num_cores),
        shutdown_barrier: Barrier::new(num_cores),
    })
}

pub fn scheduler() -> &'static Scheduler {
    SCHEDULER.get().expect("scheduler not initialized")
}

pub struct Scheduler {
    /// Per-CPU core scheduling data
    cores: CpuLocal<RefCell<Core>>,
    /// Per-CPU data that may be accessed by other workers
    remotes: CpuLocal<Remote>,
    /// The global run queue
    run_queue: mpsc_queue::MpscQueue<task::Header>,
    /// All tasks currently scheduled on this runtime
    owned: OwnedTasks,
    /// Coordinates idle workers
    idle: Idle,
    /// Signal to workers that they should be shutting down.
    shutdown: AtomicBool,
    /// Spin barrier used to synchronize shutdown between workers,
    /// see comments in [`Worker::shutdown`] for details.
    shutdown_barrier: Barrier,
}

/// CPU-local data
///
/// Logically this is part of the [`Worker`] struct, but is kept separate to allow access from
/// other parts of the code. Namely, we need access to the [`Core`] in [`Scheduler::schedule_task`].
struct Core {
    /// The worker-local run queue.
    run_queue: queue::Local,
    /// When a task is scheduled from a worker, it is stored in this slot. The
    /// worker will check this slot for a task **before** checking the run
    /// queue. This effectively results in the **last** scheduled task to be run
    /// next (LIFO). This is an optimization for improving locality which
    /// benefits message passing patterns and helps to reduce latency.
    lifo_slot: Option<TaskRef>,
}

/// Per-CPU state accessed by other workers
///
/// Ideally this would be part of `Local` as it has the same requirements, but `Local` is not `Sync`
/// and accessing the `steal` in [`Worker::steal_one_round`] requires `Sync` access. Therefore,
/// we split it off into its own thing.
struct Remote {
    /// This workers timer instance.
    timer: Timer,
    /// Steals tasks from this worker.
    steal: queue::Steal,
}

/// A scheduler worker
///
/// Data is stack-allocated and never migrates cpus.
pub struct Worker {
    /// A handle to the scheduler
    scheduler: &'static Scheduler,
    /// The physical CPU ID that this worker corresponds to.
    cpuid: usize,
    /// Fast **non-cryptographic** random number generator used to randomly distribute which workers
    /// to steal from.
    ///
    /// Selecting a random index when work-stealing helps ensure we don't
    /// have a situation where all idle steal from the first available worker,
    /// resulting in other workers ending up with huge queues of idle tasks while
    /// the first worker's queue is always empty.
    rng: FastRand,
    /// Counter used to track when to poll from the local queue vs. the
    /// global queue
    num_seq_local_queue_polls: u32,
    /// How often to check the global queue
    global_queue_interval: u32,
    /// True if the worker is currently searching for more work. Searching
    /// involves attempting to steal from other workers.
    is_searching: bool,
}

impl Scheduler {
    pub fn spawn<F>(&'static self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let id = task::Id::next();
        let (handle, maybe_task) = self.owned.bind(future, self, id);

        if let Some(task) = maybe_task {
            self.schedule(task);
        }

        handle
    }

    pub fn shutdown(&self) {
        if !self.shutdown.swap(true, Ordering::AcqRel) {
            // wake up all workers for shutdown
            self.idle.notify_all();
        }
    }

    pub fn cpu_local_timer(&self) -> &Timer {
        &self.remotes.get().unwrap().timer
    }

    fn schedule_task(&self, task: TaskRef) {
        if let Some(core) = self.cores.get()
            && let Ok(mut core) = core.try_borrow_mut()
        {
            self.schedule_local(core.deref_mut(), task);
        } else {
            self.schedule_remote(task);
        }
    }

    fn schedule_local(&self, core: &mut Core, task: TaskRef) {
        // Push to the LIFO slot
        let prev = mem::replace(&mut core.lifo_slot, Some(task));
        if let Some(prev) = prev {
            core.run_queue.push_back_or_overflow(prev, self);
        } else {
            return;
        }

        self.idle.notify_one();
    }

    fn schedule_remote(&self, task: TaskRef) {
        self.run_queue.enqueue(task);
        self.idle.notify_one();
    }
}

impl Schedule for &'static Scheduler {
    fn schedule(&self, task: TaskRef) {
        self.schedule_task(task);
    }
}

impl Overflow for Scheduler {
    fn push(&self, task: TaskRef) {
        self.run_queue.enqueue(task);
    }

    fn push_batch<I>(&self, iter: I)
    where
        I: Iterator<Item = TaskRef>,
    {
        self.run_queue.enqueue_many(iter);
    }
}

impl Worker {
    pub fn new(scheduler: &'static Scheduler, cpuid: usize, rng: &mut impl RngCore) -> Self {
        let (steal, run_queue) = queue::new();

        #[expect(tail_expr_drop_order, reason = "")]
        scheduler.cores.get_or(|| {
            RefCell::new(Core {
                lifo_slot: None,
                run_queue,
            })
        });

        #[expect(tail_expr_drop_order, reason = "")]
        scheduler.remotes.get_or(|| Remote {
            steal,
            timer: Timer::new(),
        });

        Self {
            scheduler,
            cpuid,
            is_searching: false,
            rng: FastRand::new(rng.next_u64()),
            num_seq_local_queue_polls: 0,
            global_queue_interval: DEFAULT_GLOBAL_QUEUE_INTERVAL,
        }
    }

    pub fn run(&mut self) {
        loop {
            while let Some(task) = self.next_task() {
                self.run_task(task);
            }

            // continue to execute tasks because we possibly unblocked some tasks now by turning
            // the timer
            if self.turn_timer() {
                continue;
            }

            if self.scheduler.shutdown.load(Ordering::Acquire) {
                break;
            }

            // if we have no tasks to run, we can sleep until an interrupt
            // occurs.
            self.scheduler.idle.transition_worker_to_waiting(self);
            // Safety: we park the
            unsafe {
                arch::cpu_park();
            }
            self.scheduler.idle.transition_worker_from_waiting(self);
        }

        self.shutdown();
    }

    fn run_task(&mut self, task: TaskRef) {
        // Make sure the worker is not in the **searching** state. This enables
        // another idle worker to try to steal work.
        if self.transition_from_searching() {
            // super::counters::inc_num_relay_search();
            self.scheduler.idle.notify_one();
        }

        let poll_result = task.poll();
        match poll_result {
            PollResult::Ready | PollResult::ReadyJoined => {
                self.scheduler.owned.remove(task);
            }
            PollResult::PendingSchedule => {
                self.scheduler.schedule_task(task);
            }
            PollResult::Pending => {}
        }
    }

    fn turn_timer(&self) -> bool {
        let (expired, next_deadline) = self.scheduler.cpu_local_timer().turn();
        if let Some(next_deadline) = next_deadline {
            riscv::sbi::time::set_timer(next_deadline.ticks.0).unwrap();
        } else {
            // Timer interrupts are always IPIs used for sleeping
            riscv::sbi::time::set_timer(u64::MAX).unwrap();
        }
        expired > 0
    }

    fn next_task(&mut self) -> Option<TaskRef> {
        let core = self.scheduler.cores.get().unwrap();
        let mut core = core.borrow_mut();

        self.num_seq_local_queue_polls += 1;

        // Every `global_queue_interval` ticks we must check the global queue
        // to ensure that tasks in the global run queue make progress too.
        // If we reached a tick where we pull from the global queue that takes precedence.
        if self.num_seq_local_queue_polls % self.global_queue_interval == 0 {
            self.num_seq_local_queue_polls = 0;

            self.turn_timer();

            if let Some(task) = self.next_remote_task() {
                return Some(task);
            }
        }

        // Now comes the "main" part of searching for the next task. We first consult our local run
        // queue for a task.
        if let Some(task) = self.next_local_task(&mut core) {
            return Some(task);
        }

        // If our local run queue is empty we try to refill it from the global run queue.
        if let Some(task) = self.next_remote_task_and_refill_queue(&mut core) {
            return Some(task);
        }

        self.transition_to_searching();

        // Even the global run queue doesn't have tasks to run! Let's see if we can steal some from
        // other workers...
        if let Some(task) = self.search_for_work(&mut core) {
            return Some(task);
        }

        self.transition_from_searching();

        // It appears the entire scheduler is out of work, there is nothing we can do
        None
    }

    fn next_local_task(&mut self, core: &mut Core) -> Option<TaskRef> {
        core.lifo_slot.take().or_else(|| core.run_queue.pop())
    }

    fn next_remote_task(&self) -> Option<TaskRef> {
        self.scheduler.run_queue.dequeue()
    }

    fn next_remote_task_and_refill_queue(&mut self, core: &mut Core) -> Option<TaskRef> {
        let max = usize::min(
            core.run_queue.remaining_slots(),
            usize::max(core.run_queue.max_capacity() / 2, 1),
        );

        let n = if self.is_searching {
            self.scheduler.run_queue.len() / self.scheduler.idle.num_searching() + 1
        } else {
            self.scheduler.run_queue.len() / (self.scheduler.cores.len() + 1)
        };

        let n = usize::min(n, max) + 1;

        let mut tasks = self.scheduler.run_queue.consume().take(n);
        let ret = tasks.next();

        // Safety: we calculated the max from the local queues remaining capacity, it can never overflow
        unsafe {
            core.run_queue.push_back_unchecked(tasks);
        }

        ret
    }

    fn search_for_work(&mut self, core: &mut Core) -> Option<TaskRef> {
        const ROUNDS: usize = 4;

        debug_assert!(core.lifo_slot.is_none());

        if !core.run_queue.can_steal() {
            return None;
        }

        let num_workers = u32::try_from(self.scheduler.cores.len()).unwrap();
        let mut backoff = Backoff::new();

        for _ in 0..ROUNDS {
            // Start from a random worker
            let start = self.rng.fastrand_n(num_workers) as usize;

            if let Some(task) = self.steal_one_round(start, core) {
                return Some(task);
            }

            // Attempt to steal from the global task queue again
            if let Some(task) = self.next_remote_task_and_refill_queue(core) {
                return Some(task);
            }

            backoff.spin();
        }

        None
    }

    fn steal_one_round(&mut self, start: usize, core: &mut Core) -> Option<TaskRef> {
        let num_workers = self.scheduler.cores.len();

        for i in 0..num_workers {
            let i = (start + i) % num_workers;

            // Don't steal from ourselves! We know we don't have work.
            if i == self.cpuid {
                continue;
            }

            let Some(steal) = self.scheduler.remotes.iter().nth(i) else {
                // The worker might not be online yet, just advance past
                continue;
            };
            if let Some(task) = steal.steal.steal_into(&mut core.run_queue) {
                return Some(task);
            }
        }

        None
    }

    /// Returns `true` if the transition was successful
    fn transition_to_searching(&mut self) -> bool {
        if !self.is_searching {
            self.scheduler.idle.try_transition_worker_to_searching(self);
        }

        self.is_searching
    }

    /// Returns `true` if another worker must be notified
    fn transition_from_searching(&mut self) -> bool {
        if !self.is_searching {
            return false;
        }

        self.is_searching = false;
        self.scheduler.idle.transition_worker_from_searching()
    }

    fn shutdown(&mut self) {
        self.scheduler.owned.close_and_shutdown_all();

        let core = self.scheduler.cores.get().unwrap();
        let mut core = core.borrow_mut();

        // Drop the LIFO task
        drop(core.lifo_slot.take());

        // Drain tasks from the local queue
        while core.run_queue.pop().is_some() {}

        // Wait for all workers
        tracing::trace!("waiting for other workers to shut down...");
        if self.scheduler.shutdown_barrier.wait().is_leader() {
            debug_assert!(self.scheduler.owned.is_empty());

            // Drain the injection queue
            //
            // We already shut down every task, so we can simply drop the tasks. We
            // cannot call `next_remote_task()` because we already hold the lock.
            //
            // safety: passing in correct `idle::Synced`
            while let Some(task) = self.scheduler.run_queue.dequeue() {
                drop(task);
            }

            tracing::trace!("scheduler shut down, bye bye...");
        }
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct TaskStub {
    hdr: task::Header,
}

impl TaskStub {
    pub const fn new() -> Self {
        Self {
            hdr: task::Header::new_static_stub(),
        }
    }
}
