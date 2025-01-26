// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! The task module.
//!
//! The task module contains the code that manages spawned tasks and provides a
//! safe API for the rest of the runtime to use. Each task in a runtime is
//! stored in an `OwnedTasks` or `LocalOwnedTasks` object.
//!
//! # Task reference types
//!
//! A task is usually referenced by multiple handles, and there are several
//! types of handles.
//!
//!  * `OwnedTask` - tasks stored in an `OwnedTasks` or `LocalOwnedTasks` are of this
//!    reference type.
//!
//!  * `JoinHandle` - each task has a `JoinHandle` that allows access to the output
//!    of the task.
//!
//!  * `Waker` - every waker for a task has this reference type. There can be any
//!    number of waker references.
//!
//!  * `Notified` - tracks whether the task is notified.
//!
//!  * `Unowned` - this task reference type is used for tasks not stored in any
//!    runtime. Mainly used for blocking tasks, but also in tests.
//!
//! The task uses a reference count to keep track of how many active references
//! exist. The `Unowned` reference type takes up two ref-counts. All other
//! reference types take up a single ref-count.
//!
//! Besides the waker type, each task has at most one of each reference type.
//!
//! # State
//!
//! The task stores its state in an atomic `usize` with various bitfields for the
//! necessary information. The state has the following bitfields:
//!
//!  * `RUNNING` - Tracks whether the task is currently being polled or cancelled.
//!    This bit functions as a lock around the task.
//!
//!  * `COMPLETE` - Is one once the future has fully completed and has been
//!    dropped. Never unset once set. Never set together with RUNNING.
//!
//!  * `NOTIFIED` - Tracks whether a Notified object currently exists.
//!
//!  * `CANCELLED` - Is set to one for tasks that should be cancelled as soon as
//!    possible. May take any value for completed tasks.
//!
//!  * `JOIN_INTEREST` - Is set to one if there exists a `JoinHandle`.
//!
//!  * `JOIN_WAKER` - Acts as an access control bit for the join handle waker. The
//!    protocol for its usage is described below.
//!
//! The rest of the bits are used for the ref-count.
//!
//! # Fields in the task
//!
//! The task has various fields. This section describes how and when it is safe
//! to access a field.
//!
//!  * The state field is accessed with atomic instructions.
//!
//!  * The `OwnedTask` reference has exclusive access to the `owned` field.
//!
//!  * The Notified reference has exclusive access to the `queue_next` field.
//!
//!  * The `owner_id` field can be set as part of construction of the task, but
//!    is otherwise immutable and anyone can access the field immutably without
//!    synchronization.
//!
//!  * If COMPLETE is one, then the `JoinHandle` has exclusive access to the
//!    stage field. If COMPLETE is zero, then the RUNNING bitfield functions as
//!    a lock for the stage field, and it can be accessed only by the thread
//!    that set RUNNING to one.
//!
//!  * The waker field may be concurrently accessed by different threads: in one
//!    thread the runtime may complete a task and *read* the waker field to
//!    invoke the waker, and in another thread the task's `JoinHandle` may be
//!    polled, and if the task hasn't yet completed, the `JoinHandle` may *write*
//!    a waker to the waker field. The `JOIN_WAKER` bit ensures safe access by
//!    multiple threads to the waker field using the following rules:
//!
//!    1. `JOIN_WAKER` is initialized to zero.
//!
//!    2. If `JOIN_WAKER` is zero, then the `JoinHandle` has exclusive (mutable)
//!       access to the waker field.
//!
//!    3. If `JOIN_WAKER` is one, then the `JoinHandle` has shared (read-only)
//!       access to the waker field.
//!
//!    4. If `JOIN_WAKER` is one and COMPLETE is one, then the runtime has shared
//!       (read-only) access to the waker field.
//!
//!    5. If the `JoinHandle` needs to write to the waker field, then the
//!       `JoinHandle` needs to (i) successfully set `JOIN_WAKER` to zero if it is
//!       not already zero to gain exclusive access to the waker field per rule
//!       2, (ii) write a waker, and (iii) successfully set `JOIN_WAKER` to one.
//!       If the `JoinHandle` unsets `JOIN_WAKER` in the process of being dropped
//!       to clear the waker field, only steps (i) and (ii) are relevant.
//!
//!    6. The `JoinHandle` can change `JOIN_WAKER` only if COMPLETE is zero (i.e.
//!       the task hasn't yet completed). The runtime can change `JOIN_WAKER` only
//!       if COMPLETE is one.
//!
//!    7. If `JOIN_INTEREST` is zero and COMPLETE is one, then the runtime has
//!       exclusive (mutable) access to the waker field. This might happen if the
//!       `JoinHandle` gets dropped right after the task completes and the runtime
//!       sets the `COMPLETE` bit. In this case the runtime needs the mutable access
//!       to the waker field to drop it.
//!
//!    Rule 6 implies that the steps (i) or (iii) of rule 5 may fail due to a
//!    race. If step (i) fails, then the attempt to write a waker is aborted. If
//!    step (iii) fails because COMPLETE is set to one by another thread after
//!    step (i), then the waker field is cleared. Once COMPLETE is one (i.e.
//!    task has completed), the `JoinHandle` will not modify `JOIN_WAKER`. After the
//!    runtime sets COMPLETE to one, it invokes the waker if there is one so in this
//!    case when a task completes the `JOIN_WAKER` bit implicates to the runtime
//!    whether it should invoke the waker or not. After the runtime is done with
//!    using the waker during task completion, it unsets the `JOIN_WAKER` bit to give
//!    the `JoinHandle` exclusive access again so that it is able to drop the waker
//!    at a later point.
//!
//! All other fields are immutable and can be accessed immutably without
//! synchronization by anyone.
//!
//! # Safety
//!
//! This section goes through various situations and explains why the API is
//! safe in that situation.
//!
//! ## Polling or dropping the future
//!
//! Any mutable access to the future happens after obtaining a lock by modifying
//! the RUNNING field, so exclusive access is ensured.
//!
//! When the task completes, exclusive access to the output is transferred to
//! the `JoinHandle`. If the `JoinHandle` is already dropped when the transition to
//! complete happens, the thread performing that transition retains exclusive
//! access to the output and should immediately drop it.
//!
//! ## Non-Send futures
//!
//! If a future is not Send, then it is bound to a `LocalOwnedTasks`.  The future
//! will only ever be polled or dropped given a `LocalNotified` or inside a call
//! to `LocalOwnedTasks::shutdown_all`. In either case, it is guaranteed that the
//! future is on the right thread.
//!
//! If the task is never removed from the `LocalOwnedTasks`, then it is leaked, so
//! there is no risk that the task is dropped on some other thread when the last
//! ref-count drops.
//!
//! ## Non-Send output
//!
//! When a task completes, the output is placed in the stage of the task. Then,
//! a transition that sets COMPLETE to true is performed, and the value of
//! `JOIN_INTEREST` when this transition happens is read.
//!
//! If `JOIN_INTEREST` is zero when the transition to COMPLETE happens, then the
//! output is immediately dropped.
//!
//! If `JOIN_INTEREST` is one when the transition to COMPLETE happens, then the
//! `JoinHandle` is responsible for cleaning up the output. If the output is not
//! Send, then this happens:
//!
//!  1. The output is created on the thread that the future was polled on. Since
//!     only non-Send futures can have non-Send output, the future was polled on
//!     the thread that the future was spawned from.
//!  2. Since `JoinHandle<Output>` is not Send if Output is not Send, the
//!     `JoinHandle` is also on the thread that the future was spawned from.
//!  3. Thus, the `JoinHandle` will not move the output across threads when it
//!     takes or drops the output.
//!
//! ## Recursive poll/shutdown
//!
//! Calling poll from inside a shutdown call or vice-versa is not prevented by
//! the API exposed by the task module, so this has to be safe. In either case,
//! the lock in the RUNNING bitfield makes the inner call return immediately. If
//! the inner call is a `shutdown` call, then the CANCELLED bit is set, and the
//! poll call will notice it when the poll finishes, and the task is cancelled
//! at that point.

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
    /// Schedule the task to run.
    fn schedule(&self, task: TaskRef);
    /// Schedule the task to run in the near future, but yield to other tasks right now.
    fn yield_now(&self, task: TaskRef);
    /// The task has completed work and is ready to be released. The scheduler
    /// should release it immediately and return it. The task module will batch
    /// the ref-dec with setting other options.
    ///
    /// If the scheduler has already released the task, then None is returned.
    fn release(&self, task: &TaskRef) -> Option<TaskRef>;
}

fn new_task<F, S>(future: F, scheduler: S, id: Id) -> (TaskRef, TaskRef, JoinHandle<F::Output>)
where
    F: Future + 'static,
    F::Output: 'static,
    S: Schedule + 'static,
{
    let (join, scheduler, owner) = TaskRef::new(future, scheduler, id);
    let join = JoinHandle::new(join);

    (owner, scheduler, join)
}
