// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::alloc::Allocator;
use core::any::type_name;
use core::panic::Location;

use crate::error::{Closed, SpawnError};
use crate::task::id::Id;
use crate::task::join_handle::JoinHandle;
use crate::task::{Task, TaskRef};

pub struct TaskBuilder<'a, S> {
    location: Option<Location<'a>>,
    name: Option<&'a str>,
    kind: &'a str,
    schedule: S,
}

impl<'a, S> TaskBuilder<'a, S>
where
    S: Fn(TaskRef) -> Result<(), Closed>,
{
    pub fn new(schedule: S) -> Self {
        Self {
            location: None,
            name: None,
            kind: "task",
            schedule,
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

    #[inline]
    #[track_caller]
    fn build<F>(&self, future: F) -> Task<F>
    where
        F: Future + Send,
        F::Output: Send,
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

        Task::new(future, id, span)
    }

    /// Attempt spawn this [`Future`] onto the executor.
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
    pub fn try_spawn<F>(&self, future: F) -> Result<JoinHandle<F::Output>, SpawnError>
    where
        F: Future + Send,
        F::Output: Send,
    {
        let task = self.build(future);
        let task = Box::try_new(task)?;
        let (task, join) = TaskRef::new_allocated(task);

        (self.schedule)(task)?;

        Ok(join)
    }

    /// Attempt spawn this [`Future`] onto the executor using a custom [`Allocator`].
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
    pub fn try_spawn_in<F, A>(
        &self,
        future: F,
        alloc: A,
    ) -> Result<JoinHandle<F::Output>, SpawnError>
    where
        F: Future + Send,
        F::Output: Send,
        A: Allocator,
    {
        let task = self.build(future);
        let task = Box::try_new_in(task, alloc)?;
        let (task, join) = TaskRef::new_allocated(task);

        (self.schedule)(task)?;

        Ok(join)
    }
}
