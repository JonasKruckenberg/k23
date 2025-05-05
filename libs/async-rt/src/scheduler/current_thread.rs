// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::scheduler::{Core, Tick};
use crate::task::{Builder, JoinHandle, Schedule, TaskPool, TaskRef, TaskStub};
use core::alloc::{AllocError, Allocator};
use core::marker::PhantomData;
use static_assertions::{assert_impl_all, assert_not_impl_any};

/// A single threaded scheduler that polls tasks on the calling thread.
///
/// This scheduler can spawn `!Send` futures with the tradeoff that it is itself `!Send` and `!Sync`.
/// For more generally useful scheduler see [`MultiThread`].
///
/// [`MultiThread`]: crate::scheduler::MultiThread
#[derive(Debug)]
pub struct CurrentThread {
    /// The core scheduling instance data for this scheduler
    core: Core,
    /// The pool of all currently alive tasks
    task_pool: TaskPool,
    _m: PhantomData<*mut u8>,
}

// The current thread scheduler must also ofc implement `Schedule` but must explicitly be `!Send` and
// `!Sync` so we cannot accidentally use it across threads.
assert_impl_all!(&'static CurrentThread: Schedule);
assert_not_impl_any!(CurrentThread: Send, Sync);

impl Schedule for &'static CurrentThread {
    fn schedule(&self, task_ref: TaskRef) {
        todo!()
    }
    fn bind(&self, task_ref: TaskRef) -> Option<TaskRef> {
        self.task_pool.bind(task_ref)
    }
}

impl CurrentThread {
    /// How many tasks will be polled for every call to [`Self::tick`].
    pub const DEFAULT_TICK_SIZE: usize = Core::DEFAULT_TICK_SIZE;

    pub const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
        // Safety: ensured by caller
        unsafe {
            Self {
                core: Core::new_with_static_stub(stub),
                task_pool: TaskPool::new(),
                _m: PhantomData,
            }
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_spawn_local<F>(&'static self, future: F) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        Builder::new(self).try_spawn_local(future)
    }

    #[inline]
    #[track_caller]
    pub fn try_spawn_local_in<F, A>(
        &'static self,
        future: F,
        alloc: A,
    ) -> Result<JoinHandle<F::Output>, AllocError>
    where
        F: Future + 'static,
        F::Output: 'static,
        A: Allocator,
    {
        Builder::new(self).try_spawn_local_in(future, alloc)
    }

    pub fn tick(&self) -> Tick {
        self.tick_n(Self::DEFAULT_TICK_SIZE)
    }

    pub fn tick_n(&self, n: usize) -> Tick {
        self.core.tick_n(n)
    }
}

#[macro_export]
macro_rules! new_current_thread_scheduler {
    () => {{
        static STUB_TASK: $crate::task::TaskStub = $crate::task::TaskStub::new();
        unsafe {
            // safety: `MultiThread::new_with_static_stub` is unsafe because
            // the stub task must not be shared with any other `MultiThread`
            // instance. because the `new_static` macro creates the stub task
            // inside the scope of the static initializer, it is guaranteed that
            // no other `MultiThread` instance can reference the `STUB_TASK`
            // static, so this is always safe.
            $crate::scheduler::CurrentThread::new_with_static_stub(&STUB_TASK)
        }
    }};
}
pub use new_current_thread_scheduler as new_current_thread;
