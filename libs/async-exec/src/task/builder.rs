// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::scheduler::Schedule;
use crate::task::{Id, JoinHandle, Task, TaskRef};
use alloc::boxed::Box;
use core::alloc::{AllocError, Allocator};
use core::any::type_name;
use core::marker::PhantomData;
use core::panic::Location;

pub struct TaskBuilder<'a, S> {
    location: Option<Location<'a>>,
    name: Option<&'a str>,
    kind: &'a str,
    _scheduler: PhantomData<S>,
}

impl<'a, S> TaskBuilder<'a, S>
where
    S: Schedule,
{
    pub(crate) fn new() -> TaskBuilder<'a, S> {
        Self {
            location: None,
            name: None,
            kind: "task",
            _scheduler: PhantomData,
        }
    }

    /// Override the name of tasks spawned by this builder.
    ///
    /// By default, tasks are unnamed.
    pub fn name(mut self, name: &'a str) -> Self {
        self.name = Some(name);
        self
    }

    /// Override the kind string of tasks spawned by this builder, this will only show up
    /// in debug messages and spans.
    ///
    /// By default, tasks are of kind `"kind"`.
    pub fn kind(mut self, kind: &'a str) -> Self {
        self.kind = kind;
        self
    }

    /// Override the source code location that will be associated with tasks spawned by this builder.
    ///
    /// By default, tasks will inherit the source code location of where they have been first spawned.
    pub fn location(mut self, location: Location<'a>) -> Self {
        self.location = Some(location);
        self
    }

    /// Attempt convert this [`Future`] into a heap allocated task.
    ///
    /// This method returns a [`TaskRef`] which can be used to spawn it onto an [`crate::executor::Executor`]
    /// and a [`JoinHandle`] which can be used to await the futures output as well as control some aspects
    /// of its runtime behaviour (such as cancelling it).
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    #[inline]
    #[track_caller]
    pub fn try_build<F>(&self, future: F) -> Result<(TaskRef, JoinHandle<F::Output>), AllocError>
    where
        F: Future + Send,
        F::Output: Send,
    {
        self.try_build_in(future, alloc::alloc::Global)
    }

    /// Attempt convert this [`Future`] into a heap allocated task using a custom [`Allocator`].
    ///
    /// This method returns a [`TaskRef`] which can be used to spawn it onto an [`crate::executor::Executor`]
    /// and a [`JoinHandle`] which can be used to await the futures output as well as control some aspects
    /// of its runtime behaviour (such as cancelling it).
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    #[inline]
    #[track_caller]
    pub fn try_build_in<F, A>(
        &self,
        future: F,
        alloc: A,
    ) -> Result<(TaskRef, JoinHandle<F::Output>), AllocError>
    where
        F: Future + Send,
        F::Output: Send,
        A: Allocator,
    {
        let id = Id::next();

        let loc = self.location.as_ref().unwrap_or(Location::caller());
        let span = tracing::trace_span!(
            "task",
            task.tid = id.as_u64(),
            task.name = ?self.name,
            task.kind = self.kind,
            task.output = %type_name::<F::Output>(),
            loc.file = loc.file(),
            loc.line = loc.line(),
            loc.col = loc.column(),
        );

        let task = Task::<F, S>::new(future, id, span);
        let task = Box::try_new_in(task, alloc)?;

        Ok(TaskRef::new_allocated(task))
    }
}
