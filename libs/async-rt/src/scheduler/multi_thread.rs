// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::task::{Builder, Header, JoinHandle, Schedule, TaskPool, TaskRef, TaskStub};
use core::alloc::{AllocError, Allocator};
use static_assertions::assert_impl_all;
use mpsc_queue::MpscQueue;

/// A work-stealing, multithreaded scheduler.
///
/// This scheduler can only be used to spawn `Futures` that implement `Send`, for `!Send` futures,
/// see [`CurrentThread`].
///
/// [`CurrentThread`]: crate::scheduler::CurrentThread
#[derive(Debug)]
pub struct MultiThread {
    /// The pool of all currently alive tasks
    task_pool: TaskPool,
    /// The global run queue
    run_queue: MpscQueue<Header>,
}

// The multithreaded scheduler ofc implement `Schedule` but must also be `Send` and `Sync` so we
// can stick it into a static
assert_impl_all!(&'static MultiThread: Schedule, Send, Sync);

impl Schedule for &'static MultiThread {
    fn schedule(&self, task_ref: TaskRef) {
        todo!()
    }
    fn bind(&self, task_ref: TaskRef) -> Option<TaskRef> {
        self.task_pool.bind(task_ref)
    }
}

impl MultiThread {
    /// How many tasks will be polled for every call to [`Self::tick`].
    pub const DEFAULT_TICK_SIZE: usize = Core::DEFAULT_TICK_SIZE;

    pub const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
        // Safety: ensured by caller
        unsafe {
            Self {
                task_pool: TaskPool::new(),
                run_queue: MpscQueue::new_with_static_stub(&stub.header),
            }
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_spawn<F>(&'static self, future: F) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        Builder::new(self).try_spawn(future)
    }

    #[inline]
    #[track_caller]
    pub fn try_spawn_in<F, A>(
        &'static self,
        future: F,
        alloc: A,
    ) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
        A: Allocator,
    {
        Builder::new(self).try_spawn_in(future, alloc)
    }

}

// pub struct Worker {
//     scheduler: &'static MultiThread,
// }
//
// impl Worker {
//     pub const fn new(scheduler: &'static MultiThread) -> Self {
//         Self { scheduler }
//     }
//
//     pub fn tick(&self) -> Tick {
//         self.tick_n(MultiThread::DEFAULT_TICK_SIZE)
//     }
//
//     pub fn tick_n(&self, n: usize) -> Tick {
//         todo!()
//     }
// }

#[macro_export]
macro_rules! new_multi_thread_scheduler {
    () => {{
        static STUB_TASK: $crate::task::TaskStub = $crate::task::TaskStub::new();
        unsafe {
            // safety: `MultiThread::new_with_static_stub` is unsafe because
            // the stub task must not be shared with any other `MultiThread`
            // instance. because the `new_static` macro creates the stub task
            // inside the scope of the static initializer, it is guaranteed that
            // no other `MultiThread` instance can reference the `STUB_TASK`
            // static, so this is always safe.
            $crate::scheduler::MultiThread::new_with_static_stub(&STUB_TASK)
        }
    }};
}
pub use new_multi_thread_scheduler as new_multi_thread;
use crate::scheduler::Core;