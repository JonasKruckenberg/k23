// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::scheduler2::atomic_cell::AtomicCell;
use crate::scheduler2::context;
use crate::scheduler2::scheduler::idle::Idle;
use crate::scheduler2::scheduler::queue::Overflow;
use crate::scheduler2::scheduler::{idle, queue, Handle, RunQueue};
use crate::scheduler2::task::{OwnedTasks, TaskRef};
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::sync::atomic::Ordering;
use core::{cmp, mem, ptr};
use sync::{Mutex, MutexGuard};

/// This is the previous default
const DEFAULT_GLOBAL_QUEUE_INTERVAL: u32 = 61;

/// Running a task may consume the core. If the core is still available when
/// running the task completes, it is returned. Otherwise, the worker will need
/// to stop processing.
type RunResult = Result<Box<Core>, ()>;
type NextTaskResult = Result<(Option<TaskRef>, Box<Core>), ()>;

/// Value picked out of thin-air. Running the LIFO slot a handful of times
/// seems sufficient to benefit from locality. More than 3 times probably is
/// overweighing. The value can be tuned in the future with data that shows
/// improvements.
const MAX_LIFO_POLLS_PER_TICK: usize = 3;

/// A scheduler worker
///
/// Data is stack-allocated and never migrates threads
pub(super) struct Worker {
    /// True if the scheduler is being shutdown
    is_shutdown: bool,
    /// Used to collect a list of workers to notify
    workers_to_notify: Vec<usize>,
}

/// Core data
///
/// Data is heap-allocated and migrates threads.
#[repr(align(128))]
pub(in crate::scheduler2) struct Core {
    /// Index holding this core's remote/shared state.
    pub(super) index: usize,
    /// When a task is scheduled from a worker, it is stored in this slot. The
    /// worker will check this slot for a task **before** checking the run
    /// queue. This effectively results in the **last** scheduled task to be run
    /// next (LIFO). This is an optimization for improving locality which
    /// benefits message passing patterns and helps to reduce latency.
    pub(super) lifo_slot: Option<TaskRef>,
    /// The worker-local run queue.
    pub(super) run_queue: queue::Local,
    pub(super) is_searching: bool,
}

/// State shared across all workers
pub struct Shared {
    /// Per-core remote state.
    pub(super) remotes: Box<[Remote]>,
    /// The global task queue producer handle.
    pub(super) run_queue: RunQueue,
    /// Collection of all active tasks spawned onto this executor.
    pub(super) owned: OwnedTasks,
    /// Coordinates idle workers
    pub(super) idle: Idle,
    /// Data synchronized by the scheduler mutex.
    pub(super) synced: Mutex<Synced>,
}

pub struct Synced {
    /// When worker is notified, it is assigned a core. The core is placed here
    /// until the worker wakes up to take it.
    pub(super) assigned_cores: Vec<Option<Box<Core>>>,
    /// Cores that have observed the shutdown signal
    ///
    /// The core is **not** placed back in the worker to avoid it from being
    /// stolen by a thread that was spawned as part of `block_in_place`.
    pub(super) shutdown_cores: Vec<Box<Core>>,
    /// Synchronized state for `Idle`.
    pub(super) idle: idle::Synced,
}

/// Used to communicate with a worker from other threads.
pub struct Remote {
    /// Steals tasks from this worker.
    pub(super) steal: queue::Steal,
}

/// Thread-local context
pub struct Context {
    /// Handle to the current scheduler
    handle: &'static Handle,
    /// Worker index
    index: usize,
    /// True when the LIFO slot is enabled
    lifo_enabled: Cell<bool>,
    /// Core data
    core: RefCell<Option<Box<Core>>>,
    // pub(crate) defer: RefCell<Vec<TaskRef>>,
}

impl Core {
    fn next_local_task(&mut self) -> Option<TaskRef> {
        self.next_lifo_task().or_else(|| self.run_queue.pop())
    }

    fn next_lifo_task(&mut self) -> Option<TaskRef> {
        self.lifo_slot.take()
    }
}

impl Shared {
    #[inline(always)]
    pub(super) fn current_task(&self) -> Option<TaskRef> {
        // let ptr = self.current_task.load(Ordering::Acquire);
        // let ptr = NonNull::new(ptr)?;
        // Some(TaskRef::clone_from_raw(ptr))
        todo!()
    }

    /// Schedule `task` for execution, adding it to this scheduler's run queue.
    #[inline]
    pub(super) fn schedule_task(&self, task: TaskRef) {
        #[allow(tail_expr_drop_order)]
        with_current(|maybe_ctx| {
            if let Some(ctx) = maybe_ctx {
                // Make sure the task is part of the **current** scheduler.
                if ptr::eq(self, &ctx.handle.shared) {
                    // And the current thread still holds a core
                    if let Some(core) = ctx.core.borrow_mut().as_mut() {
                        self.schedule_local(ctx, core, task);
                    } else {
                        // This can happen if either the core was stolen
                        // (`block_in_place`) or the notification happens from
                        // the driver.
                        // ctx.defer.borrow_mut().push(task);
                        todo!()
                    }
                    return;
                }
            }

            // Otherwise, use the global run queue.
            self.schedule_remote(task);
        })
    }

    fn schedule_local(&self, cx: &Context, core: &mut Core, task: TaskRef) {
        if cx.lifo_enabled.get() {
            // Push to the LIFO slot
            let prev = mem::replace(&mut core.lifo_slot, Some(task));
            // let prev = cx.shared().remotes[core.index].lifo_slot.swap_local(task);

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

    fn notify_parked_local(&self) {
        // super::counters::inc_num_inc_notify_local();
        self.idle.notify_local(self);
    }

    fn schedule_remote(&self, task: TaskRef) {
        // super::counters::inc_num_notify_remote();
        // self.scheduler_metrics.inc_remote_schedule_count();

        self.run_queue.push(task);

        // Notify a worker. The mutex is passed in and will be released as part
        // of the method call.
        let synced = self.synced.lock();
        self.idle.notify_remote(synced, self);
    }

    fn next_remote_task_synced(&self) -> Option<TaskRef> {
        // safety: we only have access to a valid `Synced` in this file.
        self.run_queue.pop()
    }

    pub(super) fn shutdown_core(&self, handle: &Handle, mut core: Box<Core>) {
        self.owned.close_and_shutdown_all();

        // core.stats.submit(&self.worker_metrics[core.index]);

        let mut synced = self.synced.lock();
        synced.shutdown_cores.push(core);

        self.shutdown_finalize(handle, &mut synced);
    }

    pub(super) fn shutdown_finalize(&self, handle: &Handle, synced: &mut Synced) {
        // Wait for all cores
        if synced.shutdown_cores.len() != self.remotes.len() {
            return;
        }

        debug_assert!(self.owned.is_empty());

        for mut core in synced.shutdown_cores.drain(..) {
            // Drain tasks from the local queue
            while core.next_local_task().is_some() {}
        }

        // Drain the injection queue
        //
        // We already shut down every task, so we can simply drop the tasks. We
        // cannot call `next_remote_task()` because we already hold the lock.
        //
        // safety: passing in correct `idle::Synced`
        while let Some(task) = self.next_remote_task_synced() {
            drop(task);
        }
    }
}

impl Overflow for Shared {
    fn push(&self, task: TaskRef) {
        self.run_queue.push(task);
    }

    fn push_batch<I>(&self, iter: I)
    where
        I: Iterator<Item = TaskRef>,
    {
        self.run_queue.push_batch(iter);
    }
}

macro_rules! try_task {
    ($e:expr) => {{
        let (task, core) = $e?;
        if task.is_some() {
            return Ok((task, core));
        }
        core
    }};
}

macro_rules! try_task_new_batch {
    ($w:expr, $e:expr) => {{
        let (task, mut core) = $e?;
        if task.is_some() {
            // core.stats.start_processing_scheduled_tasks(&mut $w.stats);
            return Ok((task, core));
        }
        core
    }};
}

impl Worker {
    fn run(&mut self, cx: &Context) -> RunResult {
        todo!("worker loop here")
    }
}

impl Context {
    fn shared(&self) -> &Shared {
        &self.handle.shared
    }
}

#[track_caller]
fn with_current<R>(f: impl FnOnce(Option<&Context>) -> R) -> R {
    #[allow(tail_expr_drop_order)]
    context::with_scheduler(|ctx| match ctx {
        Some(ctx) => f(Some(ctx)),
        _ => f(None),
    })
}

pub fn run(index: usize, handle: &'static Handle) {
    let mut worker = Worker {
        is_shutdown: false,
        workers_to_notify: vec![],
    };

    context::enter(handle, || {
        // Set the worker context.
        let ctx = Context {
            index,
            handle,
            core: RefCell::new(None),
            lifo_enabled: Cell::new(true),
        };

        context::set_scheduler(&ctx, || worker.run(&ctx)).unwrap();
    });
}
