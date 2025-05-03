// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! # Cancellation
//!
//! When a task is canceled through [`JoinHandle::cancel`] or [`TaskRef::cancel`], the task is
//! signaled to shut down next time it yields at an `.await` point. If the task is already idle,
//! then it will be shut down as soon as possible. In any case the task **is guaranteed to not be
//! polled again**.
//!
//! When tasks are shut down, it will stop running at whichever `.await` it has yielded at. All local
//! variables are destroyed by running their destructor. Once shutdown has completed, awaiting the
//! [`JoinHandle`] will fail with a [cancelled error].
//!
//! # Blocking & Yielding
//!
//! [As mentioned above] code running in asynchronous tasks must not perform operations that can block.
//! This is even more true in `k23` where workarounds such as tokio's `spawn_blocking` do not exist
//! and blocking a task will directly block the entire CPU from making progress! This not only means
//! busy-waiting for a resource is a terrible idea, but also running long synchronous functions.
//!
//! Actually running synchronous code is of course inevitable if you need to perform useful work,
//! but you should take care to yield back to the runtime consistently.
//!
//! ## `yield_now`
//!
//! k23's WebAssembly runtime automatically inserts yield points periodically, but when authoring
//! raw async tasks, [`yield_now`] is very useful. It allows you to yield control back to the runtime,
//! allowing another task to make progress.
//!
//! It is a good idea to insert these yield points at the beginning of loops or when some phase of work
//! is finished.
//!
//! ```
//! # #![allow(unused)]
//! # use async_rt::task::yield_now;
//! async {
//!     loop {
//!         // allow the runtime to make progress on some other tasks
//!         yield_now().await;
//!
//!         // ...
//!         // do some work
//!         // ...
//!     }
//! };
//!
//! async {
//!     // ...
//!     // some unit of work
//!     // ...
//!
//!     // allow the runtime to make progress on some other tasks
//!     yield_now().await;
//!
//!     // ...
//!     // continue with the next phase of work
//!     // ...
//! };
//! ```
//!
//! Note that async synchronization and other primitives that guard access to some resource (including time
//! and interrupt primitives) exported by this crate will correctly suspend the calling task automatically.
//!
//! [cancelled error]: JoinError::is_cancelled

mod id;
mod join_handle;
mod pool;
mod state;
mod yield_now;
mod builder;

use alloc::boxed::Box;
use cfg_if::cfg_if;
use core::alloc::{AllocError, Allocator};
use core::any::type_name;
use core::cell::UnsafeCell;
use core::mem::offset_of;
use core::panic::AssertUnwindSafe;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use core::{fmt, mem};
use state::{JoinAction, StartPollAction, State, WakeByRefAction, WakeByValAction};
use util::{non_null, CachePadded, CheckedMaybeUninit};

pub use id::Id;
pub use join_handle::{JoinError, JoinHandle};
pub(crate) use pool::TaskPool;
pub use yield_now::yield_now;
pub use builder::Builder;

/// A scheduler that can execute tasks.
///
/// This trait defines the API required for a scheduler to handle tasks from this crate. Tasks are
/// generic over this trait so we can have multiple schedulers with different strategies. This trait
/// is not intended to be publicly implemented.
pub trait Schedule: Sized + 'static {
    fn schedule(&self, task_ref: TaskRef);
    fn bind(&self, task: TaskRef) -> Option<TaskRef>;
}

/// Outcome of calling [`Task::poll`].
///
/// This type describes how to proceed with a given task, whether it needs to be rescheduled
/// or can be dropped etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollResult {
    /// The task has completed, without waking a [`JoinHandle`] waker.
    ///
    /// The scheduler can increment a counter of completed tasks, and then drop
    /// the [`TaskRef`].
    Ready,

    /// The task has completed and a [`JoinHandle`] waker has been woken.
    ///
    /// The scheduler can increment a counter of completed tasks, and then drop
    /// the [`TaskRef`].
    ReadyJoined,

    /// The task is pending, but not woken.
    ///
    /// The scheduler can drop the [`TaskRef`], as whoever intends to wake the
    /// task later is holding a clone of its [`Waker`].
    Pending,

    /// The task has woken itself during the poll.
    ///
    /// The scheduler should re-schedule the task, rather than dropping the [`TaskRef`].
    PendingSchedule,
}

/// A type-erased, reference-counted pointer to a spawned `Task`.
///
/// Once a  `Task` is spawned, it is generally pinned in memory (a requirement of [`Future`]). Instead
/// of moving `Task`s around the scheduler, we therefore use `TaskRef`s which are just pointers to the
/// pinned `Task`. `TaskRef`s are type-erased interacting with the allocated `Tasks` through their
/// `Vtable` methods. This is done to reduce the monopolization cost otherwise incurred, since futures,
/// especially ones crated through `async {}` blocks, `async` closures or `async fn` calls are all
/// treated as *unique*, *disjoint* types which would all cause separate normalizations. E.g. spawning
/// 10 futures on the runtime (which is not a lot) would cause 10 different copies of the entire runtime
/// to be compiled, obviously terrible! The `Vtable` allows us to treat all spawned futures, regardless
/// of their exact type, the same way.
///
/// `TaskRef`s are reference-counted, and the task will be deallocated when the
/// last `TaskRef` pointing to it is dropped.
#[derive(Eq, PartialEq)]
pub struct TaskRef(pub(super) NonNull<Header>);

impl TaskRef {
    /// Returns the tasks unique[^1] identifier.
    ///
    /// [^1]: Unique to all *currently running* tasks, *not* unique across spacetime. See [`Id`] for details.
    pub fn id(&self) -> Id {
        self.header().id
    }

    /// Returns `true` when this task has run to completion.
    pub fn is_complete(&self) -> bool {
        self.state()
            .load(Ordering::Acquire)
            .get(state::Snapshot::COMPLETE)
    }

    /// Cancels the task.
    pub fn cancel(&self) -> bool {
        // try to set the canceled bit.
        let canceled = self.state().cancel();

        // if the task was successfully canceled, wake it so that it can clean
        // up after itself.
        if canceled {
            tracing::trace!("woke canceled task");
            self.wake_by_ref();
        }

        canceled
    }

    pub(crate) fn clone_from_raw(ptr: NonNull<Header>) -> TaskRef {
        let this = Self(ptr);
        this.state().clone_ref();
        this
    }

    pub(crate) fn header_ptr(&self) -> NonNull<Header> {
        self.0
    }

    pub(crate) fn header(&self) -> &Header {
        // Safety: constructor ensures the pointer is always valid
        unsafe { self.0.as_ref() }
    }

    /// Returns a reference to the task's state.
    pub(crate) fn state(&self) -> &State {
        &self.header().state
    }

    pub(crate) fn wake_by_ref(&self) {
        tracing::trace!("TaskRef::wake_by_ref {self:?}");
        let wake_by_ref_fn = self.header().vtable.wake_by_ref;
        // Safety: Called through our Vtable so this access should be fine
        unsafe { wake_by_ref_fn(self.0.as_ptr().cast::<()>()) }
    }

    pub(crate) fn poll(&self) -> PollResult {
        let poll_fn = self.header().vtable.poll;
        // Safety: Called through our Vtable so this access should be fine
        unsafe { poll_fn(self.0) }
    }

    /// # Safety
    ///
    /// The caller needs to make sure that `T` is the same type as the one that this `TaskRef` was
    /// created with.
    pub(crate) unsafe fn poll_join<T>(
        &self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<T, JoinError<T>>> {
        let poll_join_fn = self.header().vtable.poll_join;
        let mut slot = CheckedMaybeUninit::<Result<T, JoinError<T>>>::uninit();

        // Safety: This is called through the Vtable and as long as the caller makes sure that the `T` is the right
        // type, this call is safe
        let result = unsafe { poll_join_fn(self.0, NonNull::from(&mut slot).cast::<()>(), cx) };

        result.map(|result| {
            if let Err(e) = result {
                let output = if e.is_completed() {
                    // Safety: if the task completed before being canceled, we can still
                    // take its output.
                    Some(unsafe { slot.assume_init_read() }?)
                } else {
                    None
                };
                Err(e.with_output(output))
            } else {
                // Safety: if the poll function returned `Ok`, we get to take the
                // output!
                unsafe { slot.assume_init_read() }
            }
        })
    }
}

impl fmt::Debug for TaskRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskRef")
            .field("id", &self.id())
            .field("addr", &self.0)
            .finish()
    }
}

impl fmt::Pointer for TaskRef {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.0, f)
    }
}

impl Clone for TaskRef {
    #[inline]
    #[track_caller]
    fn clone(&self) -> Self {
        let loc = core::panic::Location::caller();
        tracing::trace!(
            task.addr=?self.0,
            loc.file = loc.file(),
            loc.line = loc.line(),
            loc.col = loc.column(),
            "TaskRef::clone",
        );
        self.state().clone_ref();
        Self(self.0)
    }
}

impl Drop for TaskRef {
    #[inline]
    #[track_caller]
    fn drop(&mut self) {
        tracing::trace!(
            task.addr=?self.0,
            "TaskRef::drop"
        );
        if !self.state().drop_ref() {
            return;
        }

        let deallocate = self.header().vtable.deallocate;
        // Safety: as long as we're constructed from a NonNull<Header> this is safe
        unsafe {
            deallocate(self.0);
        }
    }
}

// Safety: The state protocol ensured synchronized access to the inner task
unsafe impl Send for TaskRef {}
// Safety: The state protocol ensured synchronized access to the inner task
unsafe impl Sync for TaskRef {}

/// A task.
///
/// This struct holds the various parts of a task: the [future][`Future`]
/// itself, the task's header which holds "hot" metadata about the task, as well as a reference to
/// the tasks [scheduler]. When a task is spawned, the `Task` type is placed on the heap (or wherever
/// spawned tasks are stored), and a type-erased [`TaskRef`] that points to that `Task` is returned.
/// Once a task is spawned, it is primarily interacted with via [`TaskRef`]s.
///
/// ## Vtables and Type Erasure
///
/// The `Task` struct, once spawned, is rarely interacted with directly. Because
/// a system may spawn any number of different [`Future`] types as tasks, and
/// may potentially also contain multiple types of [scheduler] and/or [task
/// storage], the scheduler and other parts of the system generally interact
/// with tasks via type-erased [`TaskRef`]s.
///
/// However, in order to actually poll a task's [`Future`], or perform other
/// operations such as deallocating a task, it is necessary to know the type of
/// the task's [`Future`] (and potentially, that of the scheduler and/or
/// storage). Therefore, operations that are specific to the task's `S`-typed
/// [scheduler], `F`-typed [`Future`] are performed via [dynamic dispatch].
///
/// [scheduler]: crate::scheduler::Scheduler
/// [dynamic dispatch]: https://en.wikipedia.org/wiki/Dynamic_dispatch
#[repr(C)]
pub(crate) struct Task<F: Future, S>(CachePadded<TaskInner<F, S>>);

#[repr(C)]
pub(crate) struct TaskInner<F: Future, S> {
    /// This must be the first field of the `Task` struct!
    pub(super) schedulable: Schedulable<S>,
    /// The future that the task is running.
    ///
    /// If `COMPLETE` is one, then the `JoinHandle` has exclusive access to this field
    /// If COMPLETE is zero, then the RUNNING bitfield functions as
    /// a lock for the stage field, and it can be accessed only by the thread
    /// that set RUNNING to one.
    pub(crate) stage: UnsafeCell<Stage<F>>,
    /// Consumer task waiting on completion of this task.
    ///
    /// This field may be access by different threads: on one cpu we may complete a task and *read*
    /// the waker field to invoke the waker, and in another thread the task's `JoinHandle` may be
    /// polled, and if the task hasn't yet completed, the `JoinHandle` may *write* a waker to the
    /// waker field. The `JOIN_WAKER` bit in the headers`state` field ensures safe access by multiple
    /// cpu to the waker field using the following rules:
    ///
    /// 1. `JOIN_WAKER` is initialized to zero.
    ///
    /// 2. If `JOIN_WAKER` is zero, then the `JoinHandle` has exclusive (mutable)
    ///    access to the waker field.
    ///
    /// 3. If `JOIN_WAKER` is one,  then the `JoinHandle` has shared (read-only) access to the waker
    ///    field.
    ///
    /// 4. If `JOIN_WAKER` is one and COMPLETE is one, then the executor has shared (read-only) access
    ///    to the waker field.
    ///
    /// 5. If the `JoinHandle` needs to write to the waker field, then the `JoinHandle` needs to
    ///    (i) successfully set `JOIN_WAKER` to zero if it is not already zero to gain exclusive access
    ///    to the waker field per rule 2, (ii) write a waker, and (iii) successfully set `JOIN_WAKER`
    ///    to one. If the `JoinHandle` unsets `JOIN_WAKER` in the process of being dropped
    ///    to clear the waker field, only steps (i) and (ii) are relevant.
    ///
    /// 6. The `JoinHandle` can change `JOIN_WAKER` only if COMPLETE is zero (i.e.
    ///    the task hasn't yet completed). The executor can change `JOIN_WAKER` only
    ///    if COMPLETE is one.
    ///
    /// 7. If `JOIN_INTEREST` is zero and COMPLETE is one, then the executor has
    ///    exclusive (mutable) access to the waker field. This might happen if the
    ///    `JoinHandle` gets dropped right after the task completes and the executor
    ///    sets the `COMPLETE` bit. In this case the executor needs the mutable access
    ///    to the waker field to drop it.
    ///
    /// Rule 6 implies that the steps (i) or (iii) of rule 5 may fail due to a
    /// race. If step (i) fails, then the attempt to write a waker is aborted. If step (iii) fails
    /// because `COMPLETE` is set to one by another thread after step (i), then the waker field is
    /// cleared. Once `COMPLETE` is one (i.e. task has completed), the `JoinHandle` will not
    /// modify `JOIN_WAKER`. After the runtime sets COMPLETE to one, it invokes the waker if there
    /// is one so in this case when a task completes the `JOIN_WAKER` bit implicates to the runtime
    /// whether it should invoke the waker or not. After the runtime is done with using the waker
    /// during task completion, it unsets the `JOIN_WAKER` bit to give the `JoinHandle` exclusive
    /// access again so that it is able to drop the waker at a later point.
    pub(crate) join_waker: UnsafeCell<Option<Waker>>,
}

#[repr(C)]
pub(crate) struct Schedulable<S> {
    /// This must be the first field of the `Schedulable` struct!
    pub(super) header: Header,
    pub(super) scheduler: S,
}

/// Either the future or the output.
#[repr(C)] // https://github.com/rust-lang/miri/issues/3780
pub(crate) enum Stage<F: Future> {
    /// The future is still pending.
    Pending(F),
    /// The future has completed, and its output is ready to be taken by a
    /// `JoinHandle`, if one exists.
    Ready(Result<F::Output, JoinError<F::Output>>),
    /// The future has completed, and the task's output has been taken or is not
    /// needed.
    Consumed,
}

impl<F, S> Task<F, S>
where
    F: Future,
    S: Schedule + 'static,
{
    const TASK_VTABLE: Vtable = Vtable {
        poll: Self::poll,
        poll_join: Self::poll_join,
        deallocate: Self::deallocate,
        wake_by_ref: Schedulable::<S>::wake_by_ref,
    };

    pub const fn new(scheduler: S, future: F, task_id: Id, span: tracing::Span) -> Self {
        let inner = TaskInner {
            schedulable: Schedulable {
                header: Header {
                    state: State::new(),
                    vtable: &Self::TASK_VTABLE,
                    id: task_id,
                    run_queue_links: mpsc_queue::Links::new(),
                    task_pool_links: linked_list::Links::new(),
                    span,
                },
                scheduler,
            },
            stage: UnsafeCell::new(Stage::Pending(future)),
            join_waker: UnsafeCell::new(None),
        };
        Self(CachePadded(inner))
    }

    /// Poll the future, returning a [`PollResult`] that indicates what the
    /// scheduler should do with the polled task.
    ///
    /// This is a type-erased function called through the task's [`Vtable`].
    ///
    /// # Safety
    ///
    /// - `ptr` must point to the [`Header`] of a task of type `Self` (i.e. the
    ///   pointed header must have the same `S`, `F`, and `STO` type parameters
    ///   as `Self`).
    unsafe fn poll(ptr: NonNull<Header>) -> PollResult {
        // Safety: ensured by caller
        unsafe {
            let this = ptr.cast::<Self>().as_ref();

            tracing::trace!(
                task.addr=?ptr,
                task.output=type_name::<F::Output>(),
                task.id=?this.id(),
                "Task::poll",
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

    /// Poll to join the task pointed to by `ptr`, taking its output if it has
    /// completed.
    ///
    /// If the task has completed, this method returns [`Poll::Ready`], and the
    /// task's output is stored at the memory location pointed to by `outptr`.
    /// This function is called by [`JoinHandle`]s o poll the task they
    /// correspond to.
    ///
    /// This is a type-erased function called through the task's [`Vtable`].
    ///
    /// # Safety
    ///
    /// - `ptr` must point to the [`Header`] of a task of type `Self` (i.e. the
    ///   pointed header must have the same `S`, `F`, and `STO` type parameters
    ///   as `Self`).
    /// - `outptr` must point to a valid `MaybeUninit<F::Output>`.
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
                "Task::poll_join"
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
                    let waker = this.0.0.join_waker.get();
                    waker.write(Some(cx.waker().clone()));
                }
                JoinAction::Reregister => {
                    let waker = (*this.0.0.join_waker.get()).as_mut().unwrap();
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

    /// Drops the task and deallocates its memory.
    ///
    /// This is a type-erased function called through the task's [`Vtable`].
    ///
    /// # Safety
    ///
    /// - `ptr` must point to the [`Header`] of a task of type `Self` (i.e. the
    ///   pointed header must have the same `S`, `F`, and `STO` type parameters
    ///   as `Self`).
    unsafe fn deallocate(ptr: NonNull<Header>) {
        // Safety: ensured by caller
        unsafe {
            let this = ptr.cast::<Self>();
            tracing::trace!(
                task.addr=?ptr,
                task.output=type_name::<F::Output>(),
                task.id=?this.as_ref().id(),
                "Task::deallocate",
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
    pub unsafe fn poll_inner(&self, mut cx: Context<'_>) -> Poll<()> {
        let _span = self.span().enter();

        // Safety: ensured by caller
        unsafe { &mut *self.0.0.stage.get() }.poll(&mut cx, *self.id())
    }

    /// Wakes the task's [`JoinHandle`], if it has one.
    ///
    /// # Safety
    ///
    /// - The caller must have exclusive access to the task's `JoinWaker`. This
    ///   is ensured by the task's state management.
    unsafe fn wake_join_waker(&self) {
        // Safety: ensured by caller
        unsafe {
            if let Some(join_waker) = (*self.0.0.join_waker.get()).take() {
                tracing::trace!("waking {join_waker:?}");
                join_waker.wake();
            } else {
                tracing::trace!("called wake_join_waker on non-existing waker");
            }
        }
    }

    unsafe fn take_output(&self, dst: NonNull<()>) {
        // Safety: ensured by caller
        unsafe {
            match mem::replace(&mut *self.0.0.stage.get(), Stage::Consumed) {
                Stage::Ready(output) => {
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

    fn id(&self) -> &Id {
        &self.0.0.schedulable.header.id
    }
    fn state(&self) -> &State {
        &self.0.0.schedulable.header.state
    }
    #[inline]
    fn span(&self) -> &tracing::Span {
        &self.0.0.schedulable.header.span
    }
}

impl<F> Stage<F>
where
    F: Future,
{
    fn poll(&mut self, cx: &mut Context<'_>, id: Id) -> Poll<()> {
        struct Guard<'a, T: Future> {
            stage: &'a mut Stage<T>,
        }
        impl<T: Future> Drop for Guard<'_, T> {
            fn drop(&mut self) {
                // If the future panics on poll, we drop it inside the panic
                // guard.
                // Safety: caller has to ensure mutual exclusion
                *self.stage = Stage::Consumed;
            }
        }

        let poll = AssertUnwindSafe(|| -> Poll<F::Output> {
            let guard = Guard { stage: self };

            // Safety: caller has to ensure mutual exclusion
            let Stage::Pending(future) = guard.stage else {
                // TODO this will be caught by the `catch_unwind` which isn't great
                unreachable!("unexpected stage");
            };

            // Safety: The caller ensures the future is pinned.
            let future = unsafe { Pin::new_unchecked(future) };
            let res = future.poll(cx);
            mem::forget(guard);
            res
        });

        cfg_if! {
            if #[cfg(target_os = "none")] {
                let result = panic_unwind::catch_unwind(poll);
            } else {
                let result = ::std::panic::catch_unwind(poll);
            }
        }

        match result {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(ready)) => {
                *self = Stage::Ready(Ok(ready));
                Poll::Ready(())
            }
            Err(err) => {
                *self = Stage::Ready(Err(JoinError::panic(id, err)));
                Poll::Ready(())
            }
        }
    }
}

impl<S: Schedule> Schedulable<S> {
    const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_waker,
        Self::wake_by_val,
        Self::wake_by_ref,
        Self::drop_waker,
    );

    fn raw_waker(this: *const Self) -> RawWaker {
        RawWaker::new(this.cast::<()>(), &Self::WAKER_VTABLE)
    }

    #[inline(always)]
    fn state(&self) -> &State {
        &self.header.state
    }

    unsafe fn schedule(this: TaskRef) {
        // Safety: ensured by caller
        unsafe {
            this.header_ptr()
                .cast::<Self>()
                .as_ref()
                .scheduler
                .schedule(this);
        }
    }

    #[inline]
    unsafe fn drop_ref(this: NonNull<Self>) {
        // Safety: ensured by caller
        unsafe {
            tracing::trace!(task.addr=?this, task.id=?this.as_ref().header.id, "Task::drop_ref");
            if !this.as_ref().state().drop_ref() {
                return;
            }

            let deallocate = this.as_ref().header.vtable.deallocate;
            deallocate(this.cast::<Header>());
        }
    }

    // === Waker vtable methods ===

    unsafe fn wake_by_val(ptr: *const ()) {
        // Safety: called through RawWakerVtable
        unsafe {
            let ptr = ptr.cast::<Self>();
            tracing::trace!(
                target: "scheduler:waker",
                {
                    task.addr = ?ptr,
                    task.tid = (*ptr).header.id.as_u64()
                },
                "Task::wake_by_val"
            );

            let this = non_null(ptr.cast_mut());
            match this.as_ref().header.state.wake_by_val() {
                WakeByValAction::Enqueue => {
                    // the task should be enqueued.
                    //
                    // in the case that the task is enqueued, the state
                    // transition does *not* decrement the reference count. this is
                    // in order to avoid dropping the task while it is being
                    // scheduled. one reference is consumed by enqueuing the task...
                    Self::schedule(TaskRef(this.cast::<Header>()));
                    // now that the task has been enqueued, decrement the reference
                    // count to drop the waker that performed the `wake_by_val`.
                    Self::drop_ref(this);
                }
                WakeByValAction::Drop => Self::drop_ref(this),
                WakeByValAction::None => {}
            }
        }
    }

    unsafe fn wake_by_ref(ptr: *const ()) {
        // Safety: called through RawWakerVtable
        unsafe {
            let this = ptr.cast::<Self>();
            tracing::trace!(
                target: "scheduler:waker",
                {
                    task.addr = ?this,
                    task.tid = (*this).header.id.as_u64()
                },
                "Task::wake_by_ref"
            );

            let this = non_null(this.cast_mut()).cast::<Self>();
            if this.as_ref().state().wake_by_ref() == WakeByRefAction::Enqueue {
                Self::schedule(TaskRef(this.cast::<Header>()));
            }
        }
    }

    unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
        // Safety: called through RawWakerVtable
        unsafe {
            let ptr = ptr.cast::<Self>();
            tracing::trace!(
                target: "scheduler:waker",
                {
                    task.addr = ?ptr,
                    task.tid = (*ptr).header.id.as_u64()
                },
                "Task::clone_waker"
            );

            (*ptr).header.state.clone_ref();
            Self::raw_waker(ptr)
        }
    }

    unsafe fn drop_waker(ptr: *const ()) {
        // Safety: called through RawWakerVtable
        unsafe {
            let ptr = ptr.cast::<Self>();
            tracing::trace!(
                target: "scheduler:waker",
                {
                    task.addr = ?ptr,
                    task.tid = (*ptr).header.id.as_u64()
                },
                "Task::drop_waker"
            );

            let this = ptr.cast_mut();
            Self::drop_ref(non_null(this));
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub(crate) struct Header {
    /// The task's state.
    ///
    /// This field is access with atomic instructions, so it's always safe to access it.
    state: State,
    /// The task vtable for this task.
    vtable: &'static Vtable,
    /// The task's ID.
    id: Id,
    /// Links to other tasks in the intrusive run queues, either the core-local run-queue or the
    /// global run-queue.
    run_queue_links: mpsc_queue::Links<Self>,
    /// Links to other tasks in the intrusive global task pool.
    task_pool_links: linked_list::Links<Self>,
    /// The [`tracing`] span for metrics & debugging purposes.
    span: tracing::Span,
}

#[derive(Debug)]
pub(crate) struct Vtable {
    /// Poll the future, returning a [`PollResult`] that indicates what the
    /// scheduler should do with the polled task.
    pub(super) poll: unsafe fn(NonNull<Header>) -> PollResult,
    /// Poll the task's `JoinHandle` for completion, storing the output at the
    /// provided [`NonNull`] pointer if the task has completed.
    ///
    /// If the task has not completed, the [`Waker`] from the provided
    /// [`Context`] is registered to be woken when the task completes.
    // Splitting this up into type aliases just makes it *harder* to understand
    // IMO...
    #[expect(clippy::type_complexity, reason = "")]
    pub(super) poll_join: unsafe fn(
        ptr: NonNull<Header>,
        outptr: NonNull<()>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), JoinError<()>>>,
    /// Drops the task and deallocates its memory.
    pub(super) deallocate: unsafe fn(NonNull<Header>),
    /// The `wake_by_ref` function from the task's [`RawWakerVTable`].
    ///
    /// This is duplicated here as it's used to wake canceled tasks when a task
    /// is canceled by a [`TaskRef`] or [`JoinHandle`].
    pub(super) wake_by_ref: unsafe fn(*const ()),
}

// Safety: tasks are always treated as pinned in memory (a requirement for polling them)
// and care has been taken below to ensure the underlying memory isn't freed as long as the
// `TaskRef` is part of the owned tasks list.
unsafe impl linked_list::Linked for Header {
    type Handle = TaskRef;

    fn into_ptr(task: Self::Handle) -> NonNull<Self> {
        let ptr = task.header_ptr();
        // converting a `TaskRef` into a pointer to enqueue it assigns ownership
        // of the ref count to the list, so we don't want to run its `Drop`
        // impl.
        mem::forget(task);
        ptr
    }
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        TaskRef(ptr)
    }
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
        // Safety: `TaskRef` is just a newtype wrapper around `NonNull<Header>`
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, task_pool_links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}

// Safety: tasks are always treated as pinned in memory (a requirement for polling them)
// and care has been taken below to ensure the underlying memory isn't freed as long as the
// `TaskRef` is part of the owned tasks list.
unsafe impl mpsc_queue::Linked for Header {
    type Handle = TaskRef;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r.header_ptr()
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        TaskRef::clone_from_raw(ptr)
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<mpsc_queue::Links<Self>>
    where
        Self: Sized,
    {
        // Safety: `TaskRef` is just a newtype wrapper around `NonNull<Header>`
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, run_queue_links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct TaskStub {
    pub(crate) header: Header,
}

impl TaskStub {
    const STATIC_STUB_VTABLE: Vtable = Vtable {
        poll: Self::stub_poll,
        poll_join: Self::stub_poll_join,
        deallocate: Self::stub_deallocate,
        wake_by_ref: Self::stub_wake_by_ref,
    };

    unsafe fn stub_poll(ptr: NonNull<Header>) -> PollResult {
        // Safety: this method should never be called
        unsafe {
            debug_assert!(ptr.as_ref().id.is_stub());
            unreachable!("stub task ({ptr:?}) should never be polled!");
        }
    }

    unsafe fn stub_poll_join(
        ptr: NonNull<Header>,
        _outptr: NonNull<()>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), JoinError<()>>> {
        // Safety: this method should never be called
        unsafe {
            debug_assert!(ptr.as_ref().id.is_stub());
            unreachable!("stub task ({ptr:?}) should never be polled!");
        }
    }

    unsafe fn stub_deallocate(ptr: NonNull<Header>) {
        // Safety: this method should never be called
        unsafe {
            debug_assert!(ptr.as_ref().id.is_stub());
            unreachable!("stub task ({ptr:p}) should never be deallocated!");
        }
    }

    unsafe fn stub_wake_by_ref(ptr: *const ()) {
        unreachable!("stub task ({ptr:p}) has no waker and should never be woken!");
    }

    pub const fn new() -> Self {
        Self {
            header: Header {
                state: State::new(),
                vtable: &Self::STATIC_STUB_VTABLE,
                id: Id::stub(),
                run_queue_links: mpsc_queue::Links::new_stub(),
                task_pool_links: linked_list::Links::new(),
                span: tracing::Span::none(),
            },
        }
    }
}
