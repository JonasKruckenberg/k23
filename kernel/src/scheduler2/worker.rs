// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::scheduler2::fast_rand::FastRand;
use crate::scheduler2::queue::Overflow;
use crate::scheduler2::task::{JoinHandle, OwnedTasks, TaskRef};
use crate::scheduler2::{queue, task};
use crate::thread_local::ThreadLocal;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::future::Future;
use core::mem;
use sync::Mutex;

pub struct Handle {
    shared: Shared,
}

impl Handle {
    pub fn new(num_cores: usize, rand: &mut impl rand::RngCore) -> Self {
        let mut cores = Vec::with_capacity(num_cores);
        let mut remotes = Vec::with_capacity(num_cores);

        for i in 0..num_cores {
            let (steal, run_queue) = queue::new();

            cores.push(Box::new(Core {
                index: i,
                run_queue,
                lifo_slot: None,
                is_searching: false,
                rand: FastRand::new(rand.next_u64()),
            }));
            remotes.push(Remote { steal });
        }

        let stub = TaskRef::new_stub();
        let run_queue = mpsc_queue::MpscQueue::new_with_stub(stub);
        #[allow(tail_expr_drop_order)]
        Self {
            shared: Shared {
                remotes: Box::new([]),
                owned: OwnedTasks::new(),
                run_queue,
                tls: Default::default(),
                available_cores: Mutex::new(cores),
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
            self.shared.schedule_task(task);
        }

        handle
    }
}

impl task::Schedule for &'static Handle {
    fn schedule(&self, task: TaskRef) {
        self.shared.schedule_task(task);
    }

    fn current_task(&self) -> Option<TaskRef> {
        todo!()
    }

    fn release(&self, task: &TaskRef) -> Option<TaskRef> {
        self.shared.owned.remove(task)
    }

    fn yield_now(&self, task: TaskRef) {
        todo!()
    }
}

const DEFAULT_GLOBAL_QUEUE_INTERVAL: u32 = 61;

type NextTaskResult = Result<(Option<TaskRef>, Box<Core>), ()>;

/// A scheduler worker
///
/// Data is stack-allocated and never migrates threads
pub struct Worker {
    /// True if the scheduler is being shutdown
    is_shutdown: bool,
    pub hartid: usize,
    /// Counter used to track when to poll from the local queue vs. the
    /// injection queue
    num_seq_local_queue_polls: u32,
    /// How often to check the global queue
    global_queue_interval: u32,
}

/// Core data
///
/// Data is heap-allocated and migrates threads.
#[repr(align(128))]
struct Core {
    /// Index holding this core's remote/shared state.
    index: usize,
    /// The worker-local run queue.
    run_queue: queue::Local,
    /// The LIFO slot
    lifo_slot: Option<TaskRef>,
    /// True if the worker is currently searching for more work. Searching
    /// involves attempting to steal from other workers.
    is_searching: bool,
    /// Fast random number generator.
    rand: FastRand,
}

/// State shared across all workers
pub struct Shared {
    /// Per-core remote state.
    remotes: Box<[Remote]>,
    /// All tasks currently scheduled on this runtime
    owned: OwnedTasks,
    /// The global run queue
    run_queue: mpsc_queue::MpscQueue<task::raw::Header>,

    tls: ThreadLocal<Context>,

    available_cores: Mutex<Vec<Box<Core>>>,
}

/// Used to communicate with a worker from other threads.
struct Remote {
    /// Steals tasks from this worker.
    pub(super) steal: queue::Steal,
}

/// Thread-local context
struct Context {
    /// Handle to the current scheduler
    handle: &'static Handle,
    /// Core data
    core: RefCell<Option<Box<Core>>>,
    /// True when the LIFO slot is enabled
    lifo_enabled: Cell<bool>,
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
    pub fn new(hartid: usize) -> Self {
        Self {
            is_shutdown: false,
            hartid,
            num_seq_local_queue_polls: 0,
            global_queue_interval: DEFAULT_GLOBAL_QUEUE_INTERVAL,
        }
    }

    pub fn run(&mut self, handle: &'static Handle) -> Result<(), ()> {
        #[allow(tail_expr_drop_order)]
        let cx = handle.shared.tls.get_or(|| Context {
            handle,
            core: RefCell::new(None),
            lifo_enabled: Cell::new(true),
        });

        // We just started up and do not have a core yet which means
        // we need to acquire one.
        let mut core = self
            .try_acquire_available_core(&cx)
            .or_else(|| self.wait_for_core())
            .ok_or(())?; // this will only fail if we're in the process of shutting down, so return an error here

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

    fn try_acquire_available_core(&mut self, cx: &Context) -> Option<Box<Core>> {
        let mut available_cores = cx.shared().available_cores.lock();
        available_cores.pop()
    }

    // Block the current hart waiting until a core becomes available.
    fn wait_for_core(&mut self) -> Option<Box<Core>> {
        todo!()
    }

    /// Get the next task to run, this encapsulates the core of the scheduling logic.
    #[expect(tail_expr_drop_order, reason = "")]
    fn next_task(&mut self, cx: &Context, mut core: Box<Core>) -> NextTaskResult {
        self.num_seq_local_queue_polls += 1;

        // Every `global_queue_interval` ticks we must check the global queue
        // to ensure that tasks in the global run queue make progress too.
        // If we reached a tick where we pull from the global queue that takes precedence.
        if self.num_seq_local_queue_polls % self.global_queue_interval == 0 {
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
            cx.shared().run_queue.len() / 2 /* TODO cx.shared().idle.num_searching() + 1 */
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

    fn search_for_work(&mut self, cx: &Context, core: Box<Core>) -> NextTaskResult {
        todo!()
    }

    fn park(&mut self, cx: &Context, core: Box<Core>) -> NextTaskResult {
        todo!()
    }

    fn run_task(
        &mut self,
        cx: &Context,
        mut core: Box<Core>,
        task: TaskRef,
    ) -> Result<Box<Core>, ()> {
        if self.transition_from_searching(cx, &mut core) {
            // cx.shared().notify_parked_local();
        }

        task.poll();

        //      - TODO ensure we stay in our scheduling budget
        //          - poll the LIFO task

        Ok(core)
    }

    /// Returns `true` if another worker must be notified
    fn transition_from_searching(&self, cx: &Context, core: &mut Core) -> bool {
        if !core.is_searching {
            return false;
        }

        core.is_searching = false;
        // TODO cx.shared().idle.transition_worker_from_searching()
        todo!()
    }
}

impl Shared {
    fn schedule_task(&self, task: TaskRef) {
        if let Some(cx) = self.tls.get() {
            // And the current thread still holds a core
            if let Some(core) = cx.core.borrow_mut().as_mut() {
                self.schedule_local(cx, core, task);
            } else {
                todo!()
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

        log::warn!("TODO notify a worker");
        // todo!("notify a worker")
    }

    fn schedule_remote(&self, task: TaskRef) {
        self.run_queue.enqueue(task);

        log::warn!("TODO notify a worker");
        // todo!("notify a worker")
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
    fn shared(&self) -> &Shared {
        &self.handle.shared
    }
}
