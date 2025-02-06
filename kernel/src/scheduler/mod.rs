// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod idle;
mod yield_now;

use crate::cpu_local::CpuLocal;
use crate::scheduler::idle::Idle;
use crate::task::{JoinHandle, OwnedTasks, PollResult, Schedule, TaskRef};
use crate::time::Timer;
use crate::util::fast_rand::FastRand;
use crate::{arch, task};
use core::future::Future;
use core::mem;
use core::ops::DerefMut;
use core::sync::atomic::{AtomicBool, Ordering};
use rand::RngCore;
use sync::{Backoff, Barrier, Mutex, OnceLock};

const DEFAULT_GLOBAL_QUEUE_INTERVAL: u32 = 61;

static SCHEDULER: OnceLock<Scheduler> = OnceLock::new();

pub fn init(num_cores: usize) -> &'static Scheduler {
    static TASK_STUB: TaskStub = TaskStub::new();

    #[expect(tail_expr_drop_order, reason = "")]
    SCHEDULER.get_or_init(|| Scheduler {
        cores: CpuLocal::with_capacity(num_cores),
        owned: OwnedTasks::new(),
        run_queue: unsafe { mpsc_queue::MpscQueue::new_with_static_stub(&TASK_STUB.hdr) },
        shutdown: AtomicBool::new(false),
        idle: Idle::new(num_cores),
        shutdown_barrier: Barrier::new(num_cores),
    })
}

pub fn scheduler() -> &'static Scheduler {
    &SCHEDULER.get().unwrap()
}

pub struct Scheduler {
    cores: CpuLocal<Core>,
    /// The global run queue
    run_queue: mpsc_queue::MpscQueue<task::Header>,
    /// All tasks currently scheduled on this runtime
    owned: OwnedTasks,
    shutdown: AtomicBool,
    idle: Idle,
    shutdown_barrier: Barrier,
}

struct Core {
    run_queue: mpsc_queue::MpscQueue<task::Header>,
    lifo_slot: Mutex<Option<TaskRef>>,
    timer: Timer,
}

pub struct Worker {
    scheduler: &'static Scheduler,
    cpuid: usize,
    is_shutdown: bool,
    rng: FastRand,
    /// Counter used to track when to poll from the local queue vs. the
    /// global queue
    num_seq_local_queue_polls: u32,
    /// How often to check the global queue
    global_queue_interval: u32,
    is_stealing: bool,
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
        &self.cores.get().unwrap().timer
    }

    fn schedule_task(&self, task: TaskRef) {
        if let Some(core) = self.cores.get() {
            self.schedule_local(core, task);
        } else {
            self.schedule_remote(task);
        }
    }

    fn schedule_local(&self, core: &Core, task: TaskRef) {
        // Push to the LIFO slot
        let mut slot = core.lifo_slot.try_lock().expect("could not lock LIFO slot");
        let prev = mem::replace(slot.deref_mut(), Some(task));
        if let Some(prev) = prev {
            core.run_queue.enqueue(prev);
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

impl Worker {
    pub fn new(scheduler: &'static Scheduler, cpuid: usize, rng: &mut impl RngCore) -> Self {
        Self {
            scheduler,
            cpuid,
            is_shutdown: false,
            is_stealing: false,
            rng: FastRand::new(rng.next_u64()),
            num_seq_local_queue_polls: 0,
            global_queue_interval: DEFAULT_GLOBAL_QUEUE_INTERVAL,
        }
    }

    pub fn run(&mut self) {
        #[expect(tail_expr_drop_order, reason = "")]
        let core = self.scheduler.cores.get_or(|| {
            let stub = TaskRef::try_new_stub_in(alloc::alloc::Global).unwrap();
            let run_queue = mpsc_queue::MpscQueue::new_with_stub(stub);

            Core {
                run_queue,
                lifo_slot: Mutex::new(None),
                timer: Timer::new(),
            }
        });

        loop {
            while let Some(task) = self.next_task(core)
                && !self.is_shutdown
            {
                self.run_task(task);
            }

            let (expired, next_deadline) = core.timer.turn();
            if let Some(next_deadline) = next_deadline {
                riscv::sbi::time::set_timer(next_deadline.ticks.0).unwrap();
            }
            // if expired is bigger than zero that means we possibly unblocked some tasks, so let's
            // try to find some work again
            if expired > 0 {
                continue;
            }

            if self.scheduler.shutdown.load(Ordering::Acquire) {
                break;
            }

            // if we have no tasks to run, we can sleep until an interrupt
            // occurs.
            self.scheduler.idle.transition_worker_to_waiting(&self);
            unsafe {
                arch::cpu_park();
            }
            self.scheduler.idle.transition_worker_from_waiting(&self);
        }

        self.shutdown_finalize(core);
    }

    fn run_task(&mut self, task: TaskRef) {
        // Make sure the worker is not in the **searching** state. This enables
        // another idle worker to try to steal work.
        if self.transition_from_stealing() {
            // super::counters::inc_num_relay_search();
            self.scheduler.idle.notify_one();
        }

        let poll_result = task.poll();
        match poll_result {
            PollResult::Ready | PollResult::ReadyJoined => {}
            PollResult::PendingSchedule => {
                self.scheduler.schedule_task(task);
            }
            PollResult::Pending => {}
        }
    }

    fn next_task(&mut self, core: &Core) -> Option<TaskRef> {
        self.num_seq_local_queue_polls += 1;

        // Every `global_queue_interval` ticks we must check the global queue
        // to ensure that tasks in the global run queue make progress too.
        // If we reached a tick where we pull from the global queue that takes precedence.
        if self.num_seq_local_queue_polls % self.global_queue_interval == 0 {
            self.num_seq_local_queue_polls = 0;

            if let Some(task) = self.next_remote_task() {
                return Some(task);
            }
        }

        // Now comes the "main" part of searching for the next task. We first consult our local run
        // queue for a task.
        if let Some(task) = self.next_local_task(core) {
            return Some(task);
        }

        // If our local run queue is empty we try to refill it from the global run queue.
        if let Some(task) = self.next_remote_task_and_refill_queue(core) {
            return Some(task);
        }

        self.transition_to_stealing();

        // Even the global run queue doesn't have tasks to run! Let's see if we can steal some from
        // other workers...
        if let Some(task) = self.steal_work(core) {
            return Some(task);
        }

        self.transition_from_stealing();

        // It appears the entire scheduler is out of work, there is nothing we can do
        None
    }

    fn next_local_task(&self, core: &Core) -> Option<TaskRef> {
        self.next_lifo_task(core)
            .or_else(|| core.run_queue.dequeue())
    }

    fn next_lifo_task(&self, core: &Core) -> Option<TaskRef> {
        let mut slot = core.lifo_slot.try_lock().expect("could not lock LIFO slot");
        slot.take()
    }

    fn next_remote_task(&self) -> Option<TaskRef> {
        self.scheduler.run_queue.dequeue()
    }

    fn next_remote_task_and_refill_queue(&self, core: &Core) -> Option<TaskRef> {
        try_move_half(&self.scheduler.run_queue, &core.run_queue)
    }

    fn steal_work(&mut self, core: &Core) -> Option<TaskRef> {
        const ROUNDS: usize = 4;

        debug_assert!(core.lifo_slot.lock().is_none());
        // debug_assert!(core.run_queue_len.load());

        let num_workers = u32::try_from(self.scheduler.cores.len()).unwrap();
        let mut backoff = Backoff::new();

        for _ in 0..ROUNDS {
            // Start from a random worker
            let start = self.rng.fastrand_n(num_workers) as usize;

            if let Some(task) = self.steal_one_round(core, start) {
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

    fn steal_one_round(&self, core: &Core, start: usize) -> Option<TaskRef> {
        let num_workers = self.scheduler.cores.len();

        for i in 0..num_workers {
            let i = (start + i) % num_workers;

            // Don't steal from ourselves! We know we don't have work.
            if i == self.cpuid {
                continue;
            }

            let target = self.scheduler.cores.iter().nth(i).unwrap();
            if let Some(task) = try_move_half(&target.run_queue, &core.run_queue) {
                return Some(task);
            }
        }

        None
    }

    /// Returns `true` if the transition was successful
    fn transition_to_stealing(&mut self) -> bool {
        if !self.is_stealing {
            self.scheduler.idle.try_transition_worker_to_stealing(self);
        }

        self.is_stealing
    }

    /// Returns `true` if another worker must be notified
    fn transition_from_stealing(&mut self) -> bool {
        if !self.is_stealing {
            return false;
        }

        self.is_stealing = false;
        self.scheduler.idle.transition_worker_from_stealing()
    }

    fn shutdown_finalize(&mut self, core: &Core) {
        self.scheduler.owned.close_and_shutdown_all();

        // Drain tasks from the local queue
        while core.run_queue.dequeue().is_some() {}

        // Wait for all workers
        log::trace!("waiting for other workers to shut down...");
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

            log::trace!("scheduler shut down, bye bye...");
        }
    }
}

fn try_move_half(
    src: &mpsc_queue::MpscQueue<task::Header>,
    dst: &mpsc_queue::MpscQueue<task::Header>,
) -> Option<TaskRef> {
    // the div_ceil here is pretty load bearing, without it we can only move from the src queue if it
    // has 2 or more elements, and we would always have a "stuck" last task
    let n = src.len().div_ceil(2);
    let consumer = src.try_consume()?;
    let mut tasks = consumer.take(n);
    let ret = tasks.next();
    dst.enqueue_many(tasks);
    ret
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
