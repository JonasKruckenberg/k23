// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Task executor
//!
//! The task executor is responsible for scheduling and executing tasks in the k23 kernel. Tasks are
//! cooperatively scheduled and can either be kernelspace (e.g. virtual memory subsystem background tasks)
//! or userspace WASM programs. Notably this aspect of k23 is quite different to traditional OS design.
//! In addition to fully cooperative scheduling the executor also has a broader scope. For example, it
//! also takes care of managing the worker harts lifecycle and sleep states based on scheduling requirements.
//!
//! The executor is heavily inspired by [`tokio`] and the as of right now the scheduler is identical
//! to the `MultiThreadedAlt` scheduler implemented by tokio.
//!
//! # Scheduling
//!
//! All tasks in k23 are cooperatively scheduled instead of the preemptive timeslice scheduling used
//! by traditional OSes to reduce the number of heavy context switches to the absolute minimum.
//! Preemption of userspace programs is handled by compiling checkpoints into function and loop
//! headers where either an epoch or fuel based system is used to determine whether to yield.
//!
//! This approach also allows us to more fine-grained control over program execution. We can assign
//! different epoch thresholds to different programs and even adjust the threshold dynamically.
//!
//! # Time
//!
//! TODO
//!
//! # Interrupts
//!
//! TODO
//!
//! # Lifecycle of worker harts
//!
//! CPU cores available to the executor (i.e. cores that have called [`run`]) are referred to as *Workers*
//! and the scheduler will distribute spawned tasks among them. When workers are idle for a while they
//! will be put to sleep until tasks become available.
//!
//! # Detailed executor behavior
//!
//! At a very high level the executor maintains a list of tasks and while running repeatedly removes a
//! task from that list and executes it on one of the workers (by calling `poll`). When the list is
//! empty workers will be put to sleep until a task is added to the list.
//!
//! In reality the executor is of course a bit more complicated. The executor maintains one global queue,
//! and a local queue for each worker thread. The local queue of a worker thread can fit at most 256 tasks.
//! If more than 256 tasks are added to the local queue, then half of them are moved to the global queue to make space.
//!
//! When choosing The runtime will prefer to choose the next task to schedule from the local queue, and will
//! only pick a task from the global queue if the local queue is empty, or if it has picked a task
//! from the local queue `global_queue_interval` times in a row.
//!
//! If both the local queue and global queue is empty, then the worker thread will attempt to steal tasks from the
//! local queue of another worker. Stealing is done by moving half of the tasks in one local queue
//! to another local queue.
//!
//! If there is no work available anywhere (not in the local queue, not in the global queue, and not by
//! stealing from other workers local queues) than the worker will park itself waiting to be woken up
//! again by an interrupt or other workers notifying it of more work.
//!
//! ## LIFO optimization
//!
//! The scheduler employs an optimization designed to improve locality which benefits message passing patterns
//! and helps to reduce latency: Each worker maintains a LIFO slot and  whenever a task wakes up another task,
//! the other task is added to the worker thread’s lifo slot instead of being added to a queue.
//! If there was already a task in the lifo slot when this happened, then the lifo slot is replaced,
//! and the task that used to be in the lifo slot is placed in the thread’s local queue.
//! When the runtime finishes scheduling a task, it will schedule the task in the lifo slot immediately, if any.
//!  Furthermore, if a worker thread uses the lifo slot three times in a row, it is temporarily disabled until the worker
//! thread has scheduled a task that didn’t come from the lifo slot.
//!
//! [`tokio`]: https://github.com/tokio-rs/tokio

mod queue;
mod scheduler;
mod task;
mod yield_now;

use core::future::Future;
use rand::RngCore;
use sync::OnceLock;
pub use task::JoinHandle;
use crate::executor::task::TaskRef;

static EXECUTOR: OnceLock<Executor> = OnceLock::new();

pub struct Executor {
    /// Handle to the scheduler used by this runtime
    // If we ever want to support multiple runtimes, this should become an enum over the different
    // variants. For now, we only support the multithreaded scheduler.
    scheduler: scheduler::multi_thread::Handle,
}

/// Get a reference to the current executor.
pub fn current() -> &'static Executor {
    EXECUTOR.get().expect("executor not initialized")
}

/// Initialize the global executor.
///
/// This will allocate required state for `num_cores` of harts. Tasks can immediately be spawned
/// using the returned runtime reference (a reference to the runtime can also be obtained using
/// [`current()`]) but no tasks will run until at least one hart in the system enters its
/// runtime loop using [`run()`].
#[cold]
pub fn init(num_cores: usize, rng: &mut impl RngCore, shutdown_on_idle: bool) -> &'static Executor {
    #[expect(tail_expr_drop_order, reason = "")]
    EXECUTOR.get_or_init(|| Executor {
        scheduler: scheduler::multi_thread::Handle::new(num_cores, rng, shutdown_on_idle),
    })
}

/// Run the async runtime loop on the calling hart.
///
/// This function will not return until the runtime is shut down.
#[inline]
pub fn run(
    rt: &'static Executor,
    hartid: usize,
    initial: impl FnOnce()
) -> Result<(), ()> {
    scheduler::multi_thread::worker::run(&rt.scheduler, hartid, initial)
}

impl Executor {
    /// Spawns a future onto the async runtime.
    ///
    /// The returned [`JoinHandle`] can be used to await the result of the future or cancel it.
    pub fn spawn<F>(&'static self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.scheduler.spawn(future)
    }

    pub fn shutdown(&'static self) {
        self.scheduler.shutdown();
    }
}
