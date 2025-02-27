// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::id::Id;
use super::join_handle::JoinError;
use super::state::{JoinAction, StartPollAction, State};
use super::{Header, PollResult, Schedulable, Schedule, Vtable};
use crate::utils::cache_padded::cache_padded;
use alloc::boxed::Box;
use core::any::type_name;
use core::cell::UnsafeCell;
use core::mem;
use core::mem::MaybeUninit;
use core::panic::AssertUnwindSafe;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::{Context, Poll, Waker};
use crate::utils::maybe_uninit::CheckedMaybeUninit;

cache_padded! {
    #[repr(C)]
    pub struct KernelTask<F: Future, S> {
        // This must be the first field of the `KernelTask` struct!
        schedulable: Schedulable<S>,
        /// The future that the task is running.
        ///
        /// If `COMPLETE` is one, then the `JoinHandle` has exclusive access to this field
        /// If COMPLETE is zero, then the RUNNING bitfield functions as
        /// a lock for the stage field, and it can be accessed only by the thread
        /// that set RUNNING to one.
        future_or_output: UnsafeCell<FutureOrOutput<F>>,
    }
}

/// Either the future or the output.
pub(crate) enum FutureOrOutput<F: Future> {
    /// The future is still pending.
    Pending(F),
    /// The future has completed, and its output is ready to be taken by a
    /// `JoinHandle`, if one exists.
    Ready(Result<F::Output, JoinError<F::Output>>),
    /// The future has completed, and the task's output has been taken or is not
    /// needed.
    Consumed,
}

impl<F, S> KernelTask<F, S>
where
    F: Future,
    S: Schedule + 'static,
{
    const VTABLE: Vtable = Vtable {
        poll: Self::poll,
        poll_join: Self::poll_join,
        deallocate: Self::deallocate,
        wake_by_ref: Schedulable::<S>::wake_by_ref,
    };

    pub const fn new(future: F, scheduler: S, task_id: Id, span: tracing::Span) -> Self {
        Self {
            schedulable: Schedulable::new(scheduler, task_id, &Self::VTABLE, span),
            future_or_output: UnsafeCell::new(FutureOrOutput::Pending(future)),
        }
    }

    #[inline]
    fn id(&self) -> &Id {
        &self.schedulable.header.id
    }
    #[inline]
    fn state(&self) -> &State {
        &self.schedulable.header.state
    }
    #[inline]
    fn span(&self) -> &tracing::Span {
        &self.schedulable.header.span
    }

    unsafe fn poll(ptr: NonNull<Header>) -> PollResult {
        // Safety: ensured by caller
        unsafe {
            let this = ptr.cast::<Self>().as_ref();

            tracing::trace!(
                task.addr=?ptr,
                task.output=type_name::<F::Output>(),
                task.id=?this.id(),
                "KernelTask::poll",
            );

            match this.state().start_poll() {
                // Successfully to transitioned to `POLLING` all is good!
                StartPollAction::Poll => {}
                // Something isn't right, we shouldn't poll the task right now...
                StartPollAction::DontPoll => {
                    tracing::warn!(task.addr=?ptr, "failed to transition to polling",);
                    return PollResult::Ready;
                }
                StartPollAction::Cancelled { wake_join_waker } => {
                    tracing::trace!(task.addr=?ptr, "task cancelled");
                    if wake_join_waker {
                        this.schedulable.wake_join_waker();
                        return PollResult::ReadyJoined;
                    } else {
                        return PollResult::Ready;
                    }
                }
            }

            // wrap the waker in `ManuallyDrop` because we're converting it from an
            // existing task ref, rather than incrementing the task ref count. if
            // this waker is consumed during the poll, we don't want to decrement
            // its ref count when the poll ends.
            let waker = {
                let raw = Schedulable::<S>::raw_waker(ptr.as_ptr().cast());
                mem::ManuallyDrop::new(Waker::from_raw(raw))
            };

            // actually poll the task
            let poll = {
                let cx = Context::from_waker(&waker);
                this.poll_inner(cx)
            };

            let result = this.state().end_poll(poll.is_ready());

            // if the task is ready and has a `JoinHandle` to wake, wake the join
            // waker now.
            if result == PollResult::ReadyJoined {
                this.schedulable.wake_join_waker();
            }

            result
        }
    }

    unsafe fn poll_join(
        ptr: NonNull<Header>,
        outptr: NonNull<()>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), JoinError<()>>> {
        // Safety: ensured by caller
        unsafe {
            let this = ptr.cast::<Self>().as_ref();
            tracing::trace!(
                task.addr=?ptr,
                task.output=type_name::<F::Output>(),
                task.id=?this.id(),
                "KernelTask::poll_join"
            );

            match this.state().try_join() {
                JoinAction::TakeOutput => {
                    // safety: if the state transition returns
                    // `JoinAction::TakeOutput`, this indicates that we have
                    // exclusive permission to read the task output.
                    this.take_output(outptr);
                    return Poll::Ready(Ok(()));
                }
                JoinAction::Canceled { completed } => {
                    // if the task has completed before it was canceled, also try to
                    // read the output, so that it can be returned in the `JoinError`.
                    if completed {
                        // safety: if the state transition returned `Canceled`
                        // with `completed` set, this indicates that we have
                        // exclusive permission to take the output.
                        this.take_output(outptr);
                    }
                    return Poll::Ready(Err(JoinError::cancelled(completed, *this.id())));
                }
                JoinAction::Register => {
                    let waker = &mut *this.schedulable.header.join_waker.get();
                    waker.write(cx.waker().clone());
                }
                JoinAction::Reregister => {
                    let waker = (*this.schedulable.header.join_waker.get()).assume_init_mut();
                    let new_waker = cx.waker();
                    if !waker.will_wake(new_waker) {
                        *waker = new_waker.clone();
                    }
                }
            }
            this.state().join_waker_registered();
            Poll::Pending
        }
    }

    unsafe fn deallocate(ptr: NonNull<Header>) {
        // Safety: ensured by caller
        unsafe {
            let this = ptr.cast::<Self>();
            tracing::trace!(
                task.addr=?ptr,
                task.output=type_name::<F::Output>(),
                task.id=?this.as_ref().id(),
                "KernelTask::deallocate",
            );
            debug_assert_eq!(
                ptr.as_ref().state.load(Ordering::Acquire).ref_count(),
                0,
                "a task may not be deallocated if its ref count is greater than zero!"
            );
            drop(Box::from_raw(this.as_ptr()));
        }
    }

    /// Polls the future. If the future completes, the output is written to the
    /// stage field.
    ///
    /// # Safety
    ///
    /// The caller has to ensure this cpu has exclusive mutable access to the tasks `stage` field (ie the
    /// future or output).
    unsafe fn poll_inner(&self, mut cx: Context<'_>) -> Poll<()> {
        let _span = self.span().enter();

        // Safety: ensured by caller
        unsafe { &mut *self.future_or_output.get() }.poll(&mut cx, *self.id())
    }

    unsafe fn take_output(&self, dst: NonNull<()>) {
        // Safety: ensured by caller
        unsafe {
            match mem::replace(&mut *self.future_or_output.get(), FutureOrOutput::Consumed) {
                FutureOrOutput::Ready(output) => {
                    // let output = self.stage.take_output();
                    // safety: the caller is responsible for ensuring that this
                    // points to a `MaybeUninit<F::Output>`.
                    let dst = dst
                        .cast::<CheckedMaybeUninit<Result<F::Output, JoinError<F::Output>>>>()
                        .as_mut();

                    // that's right, it goes in the `NonNull<()>` hole!
                    dst.write(output);
                }
                _ => panic!("JoinHandle polled after completion"),
            }
        }
    }
}

impl<F> FutureOrOutput<F>
where
    F: Future,
{
    fn poll(&mut self, cx: &mut Context<'_>, id: Id) -> Poll<()> {
        struct Guard<'a, T: Future> {
            stage: &'a mut FutureOrOutput<T>,
        }
        impl<T: Future> Drop for Guard<'_, T> {
            fn drop(&mut self) {
                // If the future panics on poll, we drop it inside the panic
                // guard.
                // Safety: caller has to ensure mutual exclusion
                *self.stage = FutureOrOutput::Consumed;
            }
        }

        // Poll the future.
        let result = panic::catch_unwind(AssertUnwindSafe(|| -> Poll<F::Output> {
            let guard = Guard { stage: self };

            // Safety: caller has to ensure mutual exclusion
            let FutureOrOutput::Pending(future) = guard.stage else {
                // TODO this will be caught by the `catch_unwind` which isn't great
                unreachable!("unexpected stage");
            };

            // Safety: The caller ensures the future is pinned.
            let future = unsafe { Pin::new_unchecked(future) };
            let res = future.poll(cx);
            mem::forget(guard);
            res
        }));

        match result {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(ready)) => {
                *self = FutureOrOutput::Ready(Ok(ready));
                Poll::Ready(())
            }
            Err(err) => {
                *self = FutureOrOutput::Ready(Err(JoinError::panic(id, err)));
                Poll::Ready(())
            }
        }
    }
}
