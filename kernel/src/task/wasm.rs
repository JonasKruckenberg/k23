// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};
use crate::task::id::Id;
use crate::task::join_handle::JoinError;
use crate::task::state::{JoinAction, StartPollAction, State};
use super::{Header, PollResult, Schedulable, Schedule, Vtable};
use crate::utils::cache_padded::cache_padded;

cache_padded! {
    #[repr(C)]
    pub struct WasmTask<S> {
        // This must be the first field of the `WasmTask` struct!
        schedulable: Schedulable<S>,
        /// The address space this task belongs to
        address_space: (),
        /// The stack used to execute this WASM task on
        stack: (),
        /// The VMContext this task belongs to
        vmctx: (),
        /// The VMContext offset
        vmoffsets: ()
    }
}

impl<S> WasmTask<S>
where
    S: Schedule + 'static,
{
    const VTABLE: Vtable = Vtable {
        poll: Self::poll,
        poll_join: Self::poll_join,
        deallocate: Self::deallocate,
        wake_by_ref: Schedulable::<S>::wake_by_ref,
    };

    pub const fn new(scheduler: S, task_id: Id, span: tracing::Span) -> Self {
        Self {
            schedulable: Schedulable::new(scheduler, task_id, &Self::VTABLE, span),

            address_space: (),
            stack: (),
            vmctx: (),
            vmoffsets: (),
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

    #[tracing::instrument]
    unsafe fn poll(ptr: NonNull<Header>) -> PollResult {
        // Safety: ensured by caller
        unsafe {
            let this = ptr.cast::<Self>().as_ref();

            tracing::trace!(
                task.addr=?ptr,
                task.id=?this.id(),
                "WasmTask::poll",
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
                        this.wake_join_waker();
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
                this.wake_join_waker();
            }

            result
        }
    }

    #[tracing::instrument]
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
                task.id=?this.id(),
                "WasmTask::poll_join"
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
                    waker.write(Some(cx.waker().clone()));
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

    #[tracing::instrument]
    unsafe fn deallocate(ptr: NonNull<Header>) {
        todo!()
    }

    unsafe fn poll_inner(&self, mut cx: Context<'_>) -> Poll<()> {
        todo!()
    }
}