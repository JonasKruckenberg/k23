// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod error;
mod id;
mod join_handle;
mod owned_tasks;
pub(crate) mod raw;
mod state;
mod waker;

use core::future::Future;
pub use error::JoinError;
pub use id::Id;
pub use join_handle::JoinHandle;
pub use owned_tasks::OwnedTasks;
pub use raw::TaskRef;

pub type Result<T> = core::result::Result<T, JoinError>;

pub enum PollResult {
    Complete,
    Notified,
    Done,
    Dealloc,
}

pub trait Schedule {
    fn schedule(&self, task: TaskRef);
    #[must_use]
    fn current_task(&self) -> Option<TaskRef>;
    /// The task has completed work and is ready to be released. The scheduler
    /// should release it immediately and return it. The task module will batch
    /// the ref-dec with setting other options.
    ///
    /// If the scheduler has already released the task, then None is returned.
    fn release(&self, task: &TaskRef) -> Option<TaskRef>;
    fn yield_now(&self, task: TaskRef);
}

fn new_task<F, S>(future: F, scheduler: S, id: Id) -> (TaskRef, TaskRef, JoinHandle<F::Output>)
where
    F: Future + 'static,
    F::Output: 'static,
    S: Schedule,
{
    let (join, scheduler, owner) = TaskRef::new(future, scheduler, id);
    let join = JoinHandle::new(join);

    (owner, scheduler, join)
}
