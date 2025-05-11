// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::task::{Id, JoinHandle, Schedule, Task, TaskRef};
use alloc::boxed::Box;
use core::alloc::{AllocError, Allocator};
use core::any::type_name;
use core::panic::Location;

/// Allows configuring certain aspects of tasks before spawning them onto a scheduler.
#[derive(Debug, Clone)]
pub struct TaskBuilder<'a, S> {
    scheduler: S,
    location: Option<Location<'a>>,
    name: Option<&'a str>,
    kind: &'a str,
}

impl<'a, S> TaskBuilder<'a, S> {
    pub(crate) const fn new(scheduler: S) -> Self {
        Self {
            scheduler,
            location: None,
            name: None,
            kind: "task",
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

    /// Attempt to spawn a given [`Future`] onto this builder's scheduler.
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output
    /// as well as control some aspects of its runtime behaviour (such as cancelling it).
    ///
    /// Spawning tasks on its own does nothing, the [`StaticScheduler`] must be run with [`Self::tick`] or [`Self::tick_n`]
    /// in order to actually make progress.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] when allocation of the task fails.
    #[inline]
    #[track_caller]
    pub fn try_spawn<F>(&self, future: F) -> Result<JoinHandle<F::Output>, AllocError>
    where
        S: Schedule + 'static,
        F: Future + 'static,
        F::Output: 'static,
    {
        self.try_spawn_in(future, alloc::alloc::Global)
    }

    /// Attempt to spawn a given [`Future`] onto this builder's scheduler.
    ///
    /// Unlike `Self::try_spawn` this will attempt to allocate the task on the provided allocator
    /// instead of the default global one.
    ///
    /// This method returns a [`JoinHandle`] which can be used to await the futures output
    /// as well as control some aspects of its runtime behaviour (such as cancelling it).
    ///
    /// Spawning tasks on its own does nothing, the [`StaticScheduler`] must be run with [`Self::tick`] or [`Self::tick_n`]
    /// in order to actually make progress.
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
    ) -> Result<JoinHandle<F::Output>, AllocError>
    where
        S: Schedule + 'static,
        F: Future + 'static,
        F::Output: 'static,
        A: Allocator,
    {
        let id = Id::next();

        let loc = self.location.as_ref().unwrap_or(Location::caller());
        let span = tracing::trace_span!(
            "scheduler.spawn",
            task.tid = id.as_u64(),
            task.name = ?self.name,
            task.kind = self.kind,
            task.output = %type_name::<F::Output>(),
            loc.file = loc.file(),
            loc.line = loc.line(),
            loc.col = loc.column(),
        );

        let task = Task::new(self.scheduler.clone(), future, id, span);
        let task = Box::try_new_in(task, alloc)?;
        let (task, join) = TaskRef::new_allocated(task);

        self.scheduler.spawn(task);

        Ok(join)
    }
}
