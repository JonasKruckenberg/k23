// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::panic;
use crate::task::Id;
use crate::task::TaskRef;
use alloc::boxed::Box;
use core::any::Any;
use core::fmt;
use core::future::Future;
use core::marker::PhantomData;
use core::panic::{RefUnwindSafe, UnwindSafe};
use core::pin::Pin;
use core::task::{Context, Poll};

pub struct JoinHandle<T> {
    state: JoinHandleState,
    id: Id,
    _p: PhantomData<T>,
}
static_assertions::assert_impl_all!(JoinHandle<()>: Send);

#[derive(Debug)]
enum JoinHandleState {
    Task(TaskRef),
    Empty,
    Error(JoinErrorKind),
}

pub struct JoinError<T> {
    kind: JoinErrorKind,
    id: Id,
    output: Option<T>,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum JoinErrorKind {
    Cancelled { completed: bool },
    Panic(Box<dyn Any + Send + 'static>),
}

// === impl JoinHandle ===

impl<T> UnwindSafe for JoinHandle<T> {}

impl<T> RefUnwindSafe for JoinHandle<T> {}

impl<T> Unpin for JoinHandle<T> {}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        // if the JoinHandle has not already been consumed, clear the join
        // handle flag on the task.
        if let JoinHandleState::Task(ref task) = self.state {
            tracing::trace!(
                state=?self.state,
                task.id=?task.id(),
                consumed=false,
                "drop JoinHandle"
            );

            task.state().drop_join_handle();
        } else {
            tracing::trace!(
                state=?self.state,
                consumed=false,
                "drop JoinHandle"
            );
        }
    }
}

impl<T> fmt::Debug for JoinHandle<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinHandle")
            .field("output", &core::any::type_name::<T>())
            .field("task", &self.state)
            .field("id", &self.id)
            .finish()
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError<T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let task = match core::mem::replace(&mut this.state, JoinHandleState::Empty) {
            JoinHandleState::Task(task) => task,
            JoinHandleState::Empty => {
                panic!("`TaskRef` only taken while polling a `JoinHandle`; this is a bug")
            }
            JoinHandleState::Error(kind) => {
                return Poll::Ready(Err(JoinError {
                    kind,
                    id: this.id,
                    output: None,
                }));
            }
        };

        // // Keep track of task budget
        // TODO let coop = ready!(crate::runtime::coop::poll_proceed(cx));

        // Safety: the `JoinHandle` must have been constructed with the
        // task's actual output type!
        let poll = unsafe { task.poll_join::<T>(cx) };

        if poll.is_pending() {
            this.state = JoinHandleState::Task(task);
        } else {
            // TODO coop.made_progress();

            // clear join interest
            task.state().drop_join_handle();
        }
        poll
    }
}

// ==== PartialEq impls for JoinHandle/TaskRef ====

impl<T> PartialEq<TaskRef> for JoinHandle<T> {
    fn eq(&self, other: &TaskRef) -> bool {
        match self.state {
            JoinHandleState::Task(ref task) => task == other,
            _ => false,
        }
    }
}

impl<T> PartialEq<&'_ TaskRef> for JoinHandle<T> {
    fn eq(&self, other: &&TaskRef) -> bool {
        match self.state {
            JoinHandleState::Task(ref task) => task == *other,
            _ => false,
        }
    }
}

impl<T> PartialEq<JoinHandle<T>> for TaskRef {
    fn eq(&self, other: &JoinHandle<T>) -> bool {
        match other.state {
            JoinHandleState::Task(ref task) => self == task,
            _ => false,
        }
    }
}

impl<T> PartialEq<&'_ JoinHandle<T>> for TaskRef {
    fn eq(&self, other: &&JoinHandle<T>) -> bool {
        match other.state {
            JoinHandleState::Task(ref task) => self == task,
            _ => false,
        }
    }
}

// ==== PartialEq impls for JoinHandle/Id ====

impl<T> PartialEq<Id> for JoinHandle<T> {
    #[inline]
    fn eq(&self, other: &Id) -> bool {
        self.id == *other
    }
}

impl<T> PartialEq<JoinHandle<T>> for Id {
    #[inline]
    fn eq(&self, other: &JoinHandle<T>) -> bool {
        *self == other.id
    }
}

impl<T> PartialEq<&'_ JoinHandle<T>> for Id {
    #[inline]
    fn eq(&self, other: &&JoinHandle<T>) -> bool {
        *self == other.id
    }
}

impl<T> JoinHandle<T> {
    pub(crate) fn new(task: TaskRef) -> Self {
        task.state().create_join_handle();

        Self {
            id: task.id(),
            state: JoinHandleState::Task(task),
            _p: PhantomData,
        }
    }

    pub fn cancel(&self) -> bool {
        match self.state {
            JoinHandleState::Task(ref task) => task.cancel(),
            _ => false,
        }
    }

    #[inline]
    #[must_use]
    pub fn is_complete(&self) -> bool {
        match self.state {
            JoinHandleState::Task(ref task) => task.is_complete(),
            // if the `JoinHandle`'s `TaskRef` has been taken, we know the
            // `Future` impl for `JoinHandle` completed, and the task has
            // _definitely_ completed.
            _ => true,
        }
    }
}

// === impl JoinError ===

impl JoinError<()> {
    pub(super) fn cancelled(completed: bool, id: Id) -> Self {
        Self {
            kind: JoinErrorKind::Cancelled { completed },
            id,
            output: None,
        }
    }

    pub(super) fn with_output<T>(self, output: Option<T>) -> JoinError<T> {
        JoinError {
            kind: self.kind,
            id: self.id,
            output,
        }
    }
}

impl<T> JoinError<T> {
    pub(super) fn panic(id: Id, err: Box<dyn Any + Send + 'static>) -> Self {
        Self {
            kind: JoinErrorKind::Panic(err),
            id,
            output: None,
        }
    }

    pub fn is_completed(&self) -> bool {
        matches!(&self.kind, JoinErrorKind::Cancelled { completed: true })
    }

    /// Returns true if the error was caused by the task being cancelled.
    ///
    /// See [the module level docs] for more information on cancellation.
    ///
    /// [the module level docs]: crate::task#cancellation
    pub fn is_cancelled(&self) -> bool {
        matches!(&self.kind, JoinErrorKind::Cancelled { .. })
    }

    /// Returns true if the error was caused by the task panicking.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::panic;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let err = tokio::spawn(async {
    ///         panic!("boom");
    ///     }).await.unwrap_err();
    ///
    ///     assert!(err.is_panic());
    /// }
    /// ```
    pub fn is_panic(&self) -> bool {
        matches!(&self.kind, JoinErrorKind::Panic(_))
    }

    /// Consumes the join error, returning the object with which the task panicked.
    ///
    /// # Panics
    ///
    /// `into_panic()` panics if the `Error` does not represent the underlying
    /// task terminating with a panic. Use `is_panic` to check the error reason
    /// or `try_into_panic` for a variant that does not panic.
    ///
    /// # Examples
    ///
    /// ```should_panic
    /// use std::panic;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let err = tokio::spawn(async {
    ///         panic!("boom");
    ///     }).await.unwrap_err();
    ///
    ///     if err.is_panic() {
    ///         // Resume the panic on the main task
    ///         panic::begin_unwind(err.into_panic());
    ///     }
    /// }
    /// ```
    #[track_caller]
    pub fn into_panic(self) -> Box<dyn Any + Send + 'static> {
        self.try_into_panic()
            .expect("`JoinError` reason is not a panic.")
    }

    /// Consumes the join error, returning the object with which the task
    /// panicked if the task terminated due to a panic. Otherwise, `self` is
    /// returned.
    ///
    /// # Examples
    ///
    /// ```should_panic
    /// use std::panic;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let err = tokio::spawn(async {
    ///         panic!("boom");
    ///     }).await.unwrap_err();
    ///
    ///     if let Ok(reason) = err.try_into_panic() {
    ///         // Resume the panic on the main task
    ///         panic::begin_unwind(reason);
    ///     }
    /// }
    /// ```
    pub fn try_into_panic(self) -> Result<Box<dyn Any + Send + 'static>, Self> {
        match self.kind {
            super::JoinErrorKind::Panic(p) => Ok(p),
            _ => Err(self),
        }
    }

    /// Returns a [task ID] that identifies the task which errored relative to
    /// other currently spawned tasks.
    ///
    /// [task ID]: Id
    pub fn id(&self) -> Id {
        self.id
    }

    /// Returns the task's output, if the task completed successfully before it
    /// was canceled.
    ///
    /// Otherwise, returns `None`.
    pub fn output(self) -> Option<T> {
        self.output
    }
}

impl<T> fmt::Display for JoinError<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            JoinErrorKind::Cancelled { completed: false } => {
                write!(fmt, "task {} was cancelled before completion", self.id)
            }
            JoinErrorKind::Cancelled { completed: true } => {
                write!(fmt, "task {} was cancelled after completion", self.id)
            }
            JoinErrorKind::Panic(p) => {
                write!(
                    fmt,
                    "task {} panicked with message {:?}",
                    self.id,
                    panic::payload_as_str(p)
                )
            }
        }
    }
}

impl<T> fmt::Debug for JoinError<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            JoinErrorKind::Cancelled { completed } => write!(
                fmt,
                "JoinError::Cancelled({:?}, completed: {completed})",
                self.id
            ),
            JoinErrorKind::Panic(p) => {
                write!(
                    fmt,
                    "JoinError::Panic({:?}, {:?}, ...)",
                    self.id,
                    panic::payload_as_str(p)
                )
            }
        }
    }
}

impl<T> core::error::Error for JoinError<T> {}
