// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::metrics::Counter;
use crate::scheduler2::idle::Idle;
use crate::scheduler2::queue::Overflow;
use crate::scheduler2::task::{OwnedTasks, TaskRef};
use crate::scheduler2::{idle, queue, task, Handle};
use crate::thread_local::ThreadLocal;
use crate::util::condvar::Condvar;
use crate::util::fast_rand::FastRand;
use crate::util::parking_spot::ParkingSpot;
use crate::{arch, counter};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, Ordering};
use core::task::Waker;
use core::time::Duration;
use core::{cmp, mem, ptr};
use sync::{Mutex, MutexGuard};

type NextTaskResult = Result<(Option<TaskRef>, Box<Core>), ()>;
const DEFAULT_GLOBAL_QUEUE_INTERVAL: u32 = 61;

static GLOBAL_QUEUE_INTERVAL: Counter = counter!("scheduler.global-queue-interval");
static NUM_NO_LOCAL_WORK: Counter = counter!("scheduler.num-no-local-work");
static NUM_REMOTE_REFILL: Counter = counter!("scheduler.num-remote-refill");
static NUM_SPIN_STALL: Counter = counter!("scheduler.num-spin-stall");
static NUM_PARKS: Counter = counter!("scheduler.num-parks");
static NUM_POLLS: Counter = counter!("scheduler.num-polls");
static NUM_NOTIFY_LOCAL: Counter = counter!("scheduler.num-notify-local");

/// A scheduler worker
///
/// Data is stack-allocated and never migrates threads
pub struct Worker {
    /// True if the scheduler is being shutdown
    is_shutdown: bool,
    hartid: usize,
    /// Counter used to track when to poll from the local queue vs. the
    /// global queue
    num_seq_local_queue_polls: u32,
    /// How often to check the global queue
    global_queue_interval: u32,
    /// Snapshot of idle core list. This helps speedup stealing
    idle_snapshot: idle::Snapshot,
    /// Used to collect a list of workers to notify
    workers_to_notify: Vec<usize>,
}

/// Core data
///
/// Data is heap-allocated and migrates threads.
#[repr(align(128))]
pub(super) struct Core {
    /// Index holding this core's remote/shared state.
    pub(super) index: usize,
    /// The worker-local run queue.
    pub(super) run_queue: queue::Local,
    /// The LIFO slot
    pub(super) lifo_slot: Option<TaskRef>,
    /// True if the worker is currently searching for more work. Searching
    /// involves attempting to steal from other workers.
    pub(super) is_searching: bool,
    /// Fast random number generator.
    pub(super) rand: FastRand,
}

/// State shared across all workers
pub(super) struct Shared {
    /// Per-core remote state.
    pub(super) remotes: Box<[Remote]>,
    /// All tasks currently scheduled on this runtime
    pub(super) owned: OwnedTasks,
    /// Data synchronized by the scheduler mutex
    pub(super) synced: Mutex<Synced>,
    /// The global run queue
    pub(super) run_queue: mpsc_queue::MpscQueue<task::raw::Header>,
    /// Coordinates idle workers
    pub(super) idle: Idle,
    /// Condition variables used for parking and unparking harts. Each hart has its
    /// own `condvar` it waits on.
    pub(super) condvars: Vec<Condvar>,
    /// Synchronization state used for parking and unparking harts. This is exclusively used
    /// in conjunction with the `condvars` to coordinate parking and unparking.
    pub(super) parking_spot: ParkingSpot,
    /// Per-hart thread-local data. Logically this is part of the [`Worker`] struct, but placed here
    /// into a TLS slot instead of stack allocated so we can access it from other places (i.e. we only
    /// need access to the scheduler handle instead of access to the workers stack which wouldn't work).
    pub(super) tls: ThreadLocal<Context>,
}

/// Various bits of shared state that are synchronized by the scheduler mutex.
pub(super) struct Synced {
    pub(super) assigned_cores: Vec<Option<Box<Core>>>,
    pub(super) idle: idle::Synced,
}

/// Used to communicate with a worker from other threads.
pub(super) struct Remote {
    /// Steals tasks from this worker.
    pub(super) steal: queue::Steal,
}

/// Thread-local context
pub(super) struct Context {
    /// Handle to the current scheduler
    handle: &'static Handle,
    /// Core data
    core: RefCell<Option<Box<Core>>>,
    /// True when the LIFO slot is enabled
    lifo_enabled: Cell<bool>,
    /// The task currently being polled by this scheduler, if it is currently
    /// polling a task.
    ///
    /// If no task is currently being polled, this will be [`ptr::null_mut`].
    current_task: AtomicPtr<task::raw::Header>,
    /// Tasks to wake after resource drivers are polled. This is mostly to
    /// handle yielded tasks.
    defer: RefCell<Vec<TaskRef>>,
}

macro_rules! try_next_task_step {
    ($e:expr) => {{
        let res: NextTaskResult = $e;
        let (task, core) = res?;
        if task.is_some() {
            return Ok((task, core));
        }
        core
    }};
}

impl Worker {
    pub fn new(handle: &'static Handle, hartid: usize) -> Self {
        Self {
            is_shutdown: false,
            hartid,
            num_seq_local_queue_polls: 0,
            global_queue_interval: DEFAULT_GLOBAL_QUEUE_INTERVAL,
            idle_snapshot: idle::Snapshot::new(&handle.shared.idle),
            workers_to_notify: Vec::with_capacity(handle.shared.remotes.len()),
        }
    }

    pub fn run(&mut self, handle: &'static Handle) -> Result<(), ()> {
        #[allow(tail_expr_drop_order)]
        let cx = handle.shared.tls.get_or(|| Context {
            handle,
            core: RefCell::new(None),
            lifo_enabled: Cell::new(true),
            current_task: AtomicPtr::new(ptr::null_mut()),
            defer: RefCell::new(Vec::with_capacity(64)),
        });

        // First try to acquire an available core
        let (maybe_task, mut core) = {
            let mut synced = cx.shared().synced.lock();

            if let Some(core) = self.try_acquire_available_core(cx, &mut synced) {
                // Try to poll a task from the global queue
                // let maybe_task = cx.shared().nex();
                (None, core)
            } else {
                // block the thread to wait for a core to be assigned to us
                self.wait_for_core(cx, synced)?
            }
        };

        if let Some(task) = maybe_task {
            core = self.run_task(cx, core, task)?;
        }

        // once we have acquired a core, we can start the scheduling loop
        while !self.is_shutdown {
            let (maybe_task, c) = self.next_task(cx, core)?;
            core = c;

            if let Some(task) = maybe_task {
                core = self.run_task(cx, core, task)?;
            } else {
                // The only reason to get `None` from `next_task` is we have
                // entered the shutdown phase.
                assert!(self.is_shutdown);
                break;
            }
        }

        // at this point we received the shutdown signal, so we need to clean up

        todo!()
    }

    fn try_acquire_available_core(
        &mut self,
        cx: &Context,
        synced: &mut Synced,
    ) -> Option<Box<Core>> {
        if let Some(mut core) = cx
            .shared()
            .idle
            .try_acquire_available_core(&mut synced.idle)
        {
            self.reset_acquired_core(cx, &mut core);
            Some(core)
        } else {
            None
        }
    }

    // Block the current hart waiting until a core becomes available.
    #[allow(tail_expr_drop_order)]
    fn wait_for_core(
        &mut self,
        cx: &Context,
        mut synced: MutexGuard<'_, Synced>,
    ) -> NextTaskResult {
        // TODO why??
        if cx.shared().idle.needs_searching() {
            if let Some(mut core) = self.try_acquire_available_core(cx, &mut synced) {
                cx.shared().idle.transition_worker_to_searching(&mut core);
                return Ok((None, core));
            }
        }

        cx.shared()
            .idle
            .transition_worker_to_parked(&mut synced, self.hartid);

        // Wait until a core is available, then exit the loop.
        let mut core = loop {
            if let Some(core) = synced.assigned_cores[self.hartid].take() {
                break core;
            }

            // // If shutting down, abort
            // if cx.shared().inject.is_closed(&synced.inject) {
            //     self.shutdown_clear_defer(cx);
            //     return Err(());
            // }

            cx.shared().condvars[self.hartid].wait(&cx.shared().parking_spot, &mut synced);
        };

        self.reset_acquired_core(cx, &mut core);

        if self.is_shutdown {
            // Currently shutting down, don't do any more work
            return Ok((None, core));
        }

        let maybe_task = self.next_remote_task_and_refill_queue(cx, &mut core);

        Ok((maybe_task, core))
    }

    /// Ensure core's state is set correctly for the worker to start using.
    fn reset_acquired_core(&mut self, cx: &Context, core: &mut Core) {
        self.global_queue_interval = DEFAULT_GLOBAL_QUEUE_INTERVAL;

        // Reset `lifo_enabled` here in case the core was previously stolen from
        // a task that had the LIFO slot disabled.
        cx.lifo_enabled.set(true);

        // At this point, the local queue should be empty
        debug_assert!(core.run_queue.is_empty());
    }

    /// Get the next task to run, this encapsulates the core of the scheduling logic.
    #[expect(tail_expr_drop_order, reason = "")]
    fn next_task(&mut self, cx: &Context, mut core: Box<Core>) -> NextTaskResult {
        self.num_seq_local_queue_polls += 1;

        // Every `global_queue_interval` ticks we must check the global queue
        // to ensure that tasks in the global run queue make progress too.
        // If we reached a tick where we pull from the global queue that takes precedence.
        if self.num_seq_local_queue_polls % self.global_queue_interval == 0 {
            GLOBAL_QUEUE_INTERVAL.increment(1);
            self.num_seq_local_queue_polls = 0;

            if let Some(task) = self.next_remote_task(cx) {
                return Ok((Some(task), core));
            }
        }

        // Now comes the "main" part of searching for the next task. We first consult our local run
        // queue for a task.
        if let Some(task) = core.run_queue.pop() {
            return Ok((Some(task), core));
        }

        // If our local run queue is empty we try to refill it from the global run queue.
        if let Some(task) = self.next_remote_task_and_refill_queue(cx, &mut core) {
            return Ok((Some(task), core));
        }

        NUM_NO_LOCAL_WORK.increment(1);

        if !cx.defer.borrow().is_empty() {
            // We are deferring tasks, so poll the resource driver and schedule
            // the deferred tasks.
            try_next_task_step!(self.park_yield(cx, core));

            panic!("what happened to the deferred tasks? 🤔");
        }

        // If that also failed to provide us with a task to run, that means either
        //      - A: Other workers have tasks left in their local queues, in which case we should steal
        //           some work from them.
        //      - B: The entire system is fully out of work (or all remaining tasks are blocked waiting for interrupts)
        //           in which case we should park the current worker.
        while !self.is_shutdown {
            // Case A, find some tasks in other workers local run queues for us.
            core = try_next_task_step!(self.search_for_work(cx, core));

            // Case B, we looked everywhere even behind the fridge and found no work. Time to wait.
            core = try_next_task_step!(self.park(cx, core));
        }

        Ok((None, core))
    }

    /// Get a single task from the global run queue.
    fn next_remote_task(&self, cx: &Context) -> Option<TaskRef> {
        if cx.shared().run_queue.is_empty() {
            return None;
        }

        cx.shared().run_queue.dequeue()
    }

    /// Get a task from the global run queue but pick up a few more tasks to refill the local queue with.
    fn next_remote_task_and_refill_queue(&self, cx: &Context, core: &mut Core) -> Option<TaskRef> {
        NUM_REMOTE_REFILL.increment(1);

        if cx.shared().run_queue.is_empty() {
            return None;
        }

        // Other threads can only **remove** tasks from the current worker's
        // `run_queue`. So, we can be confident that by the time we call
        // `run_queue.push_back` below, there will be *at least* `cap`
        // available slots in the queue.
        let max = usize::min(
            core.run_queue.remaining_slots(),
            usize::max(core.run_queue.max_capacity() / 2, 1),
        );

        let n = if core.is_searching {
            cx.shared().run_queue.len() / cx.shared().idle.num_searching() + 1
        } else {
            cx.shared().run_queue.len() / (cx.shared().remotes.len() + 1)
        };

        let n = usize::min(n, max) + 1;

        let mut tasks = cx
            .shared()
            .run_queue
            .try_consume()
            .expect("inconsistent state")
            .take(n);
        let ret = tasks.next();

        // Safety: we calculated the max from the local queues remaining capacity, it can never overflow
        unsafe {
            core.run_queue.push_back_unchecked(tasks);
        }

        ret
    }

    #[allow(tail_expr_drop_order)]
    fn search_for_work(&mut self, cx: &Context, mut core: Box<Core>) -> NextTaskResult {
        const ROUNDS: usize = 4;

        debug_assert!(core.lifo_slot.is_none());
        debug_assert!(core.run_queue.is_empty());

        if !core.run_queue.can_steal() {
            return Ok((None, core));
        }

        if !self.transition_to_searching(cx, &mut core) {
            return Ok((None, core));
        }

        let num = cx.shared().remotes.len();

        for i in 0..ROUNDS {
            // Start from a random worker
            let start = core.rand.fastrand_n(num as u32) as usize;

            if let Some(task) = self.steal_one_round(cx, &mut core, start) {
                return Ok((Some(task), core));
            }

            if let Some(task) = self.next_remote_task_and_refill_queue(cx, &mut core) {
                return Ok((Some(task), core));
            }

            if i > 0 {
                NUM_SPIN_STALL.increment(1);
                unsafe {
                    log::trace!("spin stalling for {:?}", Duration::from_micros(i as u64));
                    arch::hart_park_timeout(Duration::from_micros(i as u64));
                    log::trace!("after spin stall");
                }
            }
        }

        Ok((None, core))
    }

    fn steal_one_round(&self, cx: &Context, core: &mut Core, start: usize) -> Option<TaskRef> {
        let num = cx.shared().remotes.len();

        for i in 0..num {
            let i = (start + i) % num;

            // Don't steal from ourself! We know we don't have work.
            if i == core.index {
                continue;
            }

            // If the core is currently idle, then there is nothing to steal.
            if self.idle_snapshot.is_idle(i) {
                continue;
            }

            let target = &cx.shared().remotes[i];

            if let Some(task) = target.steal.steal_into(&mut core.run_queue) {
                return Some(task);
            }
        }

        None
    }

    #[allow(tail_expr_drop_order)]
    fn park(&mut self, cx: &Context, mut core: Box<Core>) -> NextTaskResult {
        if self.can_transition_to_parked(&mut core) {
            debug_assert!(!self.is_shutdown);

            core = try_next_task_step!(self.do_park(cx, core));
        }

        Ok((None, core))
    }

    fn do_park(&mut self, cx: &Context, mut core: Box<Core>) -> NextTaskResult {
        debug_assert!(core.run_queue.is_empty());
        // Try one last time to get tasks
        if let Some(task) = self.next_remote_task_and_refill_queue(cx, &mut core) {
            return Ok((Some(task), core));
        }

        if self.is_shutdown {
            return Ok((None, core));
        }

        // Release the core
        let mut synced = cx.shared().synced.lock();
        core.is_searching = false;
        cx.shared().idle.release_core(&mut synced, core);
        
        // Wait for a core to be assigned to us
        NUM_PARKS.increment(1);
        self.wait_for_core(cx, synced)
        
    }

    #[allow(tail_expr_drop_order)]
    fn park_yield(&mut self, cx: &Context, core: Box<Core>) -> NextTaskResult {
        // TODO poll driver

        // If there are more I/O events, schedule them.
        let (maybe_task, core) =
            self.schedule_deferred_with_core(cx, core, || cx.shared().synced.lock())?;

        Ok((maybe_task, core))
    }

    #[allow(tail_expr_drop_order)]
    fn schedule_deferred_with_core<'a>(
        &mut self,
        cx: &'a Context,
        mut core: Box<Core>,
        synced: impl FnOnce() -> MutexGuard<'a, Synced>,
    ) -> NextTaskResult {
        let mut defer = cx.defer.borrow_mut();

        // Grab a task to run next
        let task = defer.pop();

        if task.is_none() {
            return Ok((None, core));
        }

        if !defer.is_empty() {
            let mut synced = synced();

            // Number of tasks we want to try to spread across idle workers
            let num_fanout = cmp::min(defer.len(), cx.shared().idle.num_idle(&synced.idle));

            // Cap the number of threads woken up at one time. This is to limit
            // the number of no-op wakes and reduce mutext contention.
            //
            // This number was picked after some basic benchmarks, but it can
            // probably be tuned using the mean poll time value (slower task
            // polls can leverage more woken workers).
            let num_fanout = cmp::min(2, num_fanout);

            if num_fanout > 0 {
                cx.shared()
                    .run_queue
                    .enqueue_many(defer.drain(..num_fanout));

                cx.shared()
                    .idle
                    .notify_many(&mut synced, &mut self.workers_to_notify, num_fanout);
            }

            // Do not run the task while holding the lock...
            drop(synced);
        }

        // Notify any workers
        for worker in self.workers_to_notify.drain(..) {
            cx.shared().condvars[worker].notify_one(&cx.shared().parking_spot);
        }

        if !defer.is_empty() {
            // Push the rest of the tasks on the local queue
            for task in defer.drain(..) {
                core.run_queue.push_back_or_overflow(task, cx.shared());
            }

            cx.shared().notify_parked_local();
        }

        Ok((task, core))
    }

    fn run_task(
        &mut self,
        cx: &Context,
        mut core: Box<Core>,
        task: TaskRef,
    ) -> Result<Box<Core>, ()> {
        if self.transition_from_searching(cx, &mut core) {
            // super::counters::inc_num_relay_search();
            cx.shared().notify_parked_local();
        }

        NUM_POLLS.increment(1);
        task.run();

        //      - TODO ensure we stay in our scheduling budget
        //          - super::counters::inc_lifo_schedules();
        //          - super::counters::inc_lifo_capped();
        //          - super::counters::inc_num_lifo_polls();
        //          - poll the LIFO task

        Ok(core)
    }

    fn transition_to_searching(&self, cx: &Context, core: &mut Core) -> bool {
        if !core.is_searching {
            cx.shared().idle.try_transition_worker_to_searching(core);
        }

        core.is_searching
    }

    /// Returns `true` if another worker must be notified
    fn transition_from_searching(&self, cx: &Context, core: &mut Core) -> bool {
        if !core.is_searching {
            return false;
        }

        core.is_searching = false;
        cx.shared().idle.transition_worker_from_searching()
    }

    fn can_transition_to_parked(&self, core: &mut Core) -> bool {
        !self.has_tasks(core) && !self.is_shutdown
    }

    fn has_tasks(&self, core: &Core) -> bool {
        core.lifo_slot.is_some() || !core.run_queue.is_empty()
    }
}

impl Shared {
    pub(super) fn schedule_task(&self, task: TaskRef, is_yield: bool) {
        if let Some(cx) = self.tls.get() {
            // And the current thread still holds a core
            if let Some(core) = cx.core.borrow_mut().as_mut() {
                if is_yield {
                    cx.defer.borrow_mut().push(task);
                } else {
                    self.schedule_local(cx, core, task);
                }
            } else {
                cx.defer.borrow_mut().push(task);
            }
        } else {
            self.schedule_remote(task);
        }
    }

    fn schedule_local(&self, cx: &Context, core: &mut Core, task: TaskRef) {
        if cx.lifo_enabled.get() {
            // Push to the LIFO slot
            let prev = mem::replace(&mut core.lifo_slot, Some(task));
            if let Some(prev) = prev {
                core.run_queue.push_back_or_overflow(prev, self);
            } else {
                return;
            }
        } else {
            core.run_queue.push_back_or_overflow(task, self);
        }

        self.notify_parked_local();
    }

    fn schedule_remote(&self, task: TaskRef) {
        self.run_queue.enqueue(task);

        let synced = self.synced.lock();
        self.idle.notify_remote(synced, self);
    }

    fn notify_parked_local(&self) {
        NUM_NOTIFY_LOCAL.increment(1);
        self.idle.notify_local(self);
    }
}

impl Overflow for Shared {
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

impl Context {
    /// Return a [`TaskRef`] referencing the task currently being polled by
    /// this scheduler, if a task is currently being polled.
    #[must_use]
    #[inline]
    pub fn current_task(&'static self) -> Option<TaskRef> {
        let ptr = self.current_task.load(Ordering::Acquire);
        let ptr = NonNull::new(ptr)?;
        Some(TaskRef::clone_from_raw(ptr))
    }

    fn shared(&self) -> &Shared {
        &self.handle.shared
    }

    pub(crate) fn defer(&self, waker: &Waker) {
        // TODO: refactor defer across all runtimes
        waker.wake_by_ref();
    }
}
