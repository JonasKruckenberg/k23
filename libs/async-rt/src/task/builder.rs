// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::task::id::Id;
use crate::task::{Schedule, Task, TaskRef};
use alloc::boxed::Box;
use core::alloc::{AllocError, Allocator};
use core::any::type_name;
use core::panic::Location;
use core::ptr::NonNull;

#[derive(Debug, Clone)]
pub struct Builder<'a, S> {
    scheduler: S,
    location: Option<Location<'a>>,
    name: Option<&'a str>,
    kind: &'a str,
}

impl<'a, S> Builder<'a, S> {
    pub(crate) const fn new(scheduler: S) -> Self {
        Self {
            scheduler,
            location: None,
            name: None,
            kind: "task",
        }
    }

    pub fn name(mut self, name: &'a str) -> Self {
        self.name = Some(name);
        self
    }

    pub fn kind(mut self, kind: &'a str) -> Self {
        self.kind = kind;
        self
    }

    pub fn location(mut self, location: Location<'a>) -> Self {
        self.location = Some(location);
        self
    }

    #[inline]
    #[track_caller]
    pub(crate) fn build<F>(&self, future: F) -> TaskRef
    where
        S: Schedule + Copy + 'static,
        F: Future,
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

        let ptr = Box::into_raw(Box::new(Task::new(self.scheduler, future, id, span)));

        // Safety: we just allocated the ptr so it is never null
        TaskRef(unsafe { NonNull::new_unchecked(ptr).cast() })
    }

    #[inline]
    #[track_caller]
    pub(crate) fn build_in<F, A>(&self, future: F, alloc: A) -> Result<TaskRef, AllocError>
    where
        S: Schedule + Copy + 'static,
        F: Future,
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

        let ptr = Box::into_raw(Box::try_new_in(
            Task::new(self.scheduler, future, id, span),
            alloc,
        )?);

        // Safety: we just allocated the ptr so it is never null
        Ok(TaskRef(unsafe { NonNull::new_unchecked(ptr).cast() }))
    }
}
