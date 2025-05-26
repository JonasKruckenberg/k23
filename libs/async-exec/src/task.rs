// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod builder;
mod id;
mod join_handle;
mod state;
mod yield_now;

use crate::loom::{cell::UnsafeCell, sync::atomic::Ordering};
use crate::scheduler::steal::{Stealer, TryStealError};
use crate::scheduler::{Schedule, Tick};
use crate::task::state::{JoinAction, StartPollAction, State, WakeByRefAction, WakeByValAction};
use alloc::boxed::Box;
use core::alloc::Allocator;
#[cfg(debug_assertions)]
use core::any::TypeId;
use core::any::type_name;
use core::mem::offset_of;
use core::panic::AssertUnwindSafe;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use core::{fmt, mem};
use util::{CachePadded, CheckedMaybeUninit, loom_const_fn};

pub use builder::TaskBuilder;
pub use id::Id;
pub use join_handle::{JoinError, JoinHandle};
pub use yield_now::yield_now;

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
pub struct TaskRef(NonNull<Header>);

#[repr(C)]
pub struct Task<F: Future, S: Schedule>(CachePadded<TaskInner<F, S>>);

#[repr(C)]
struct TaskInner<F: Future, S: Schedule> {
    /// This must be the first field of the `Task` struct!
    schedulable: Schedulable<S>,

    /// The future that the task is running.
    ///
    /// If `COMPLETE` is one, then the `JoinHandle` has exclusive access to this field
    /// If COMPLETE is zero, then the RUNNING bitfield functions as
    /// a lock for the stage field, and it can be accessed only by the thread
    /// that set RUNNING to one.
    stage: UnsafeCell<Stage<F>>,

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
    join_waker: UnsafeCell<Option<Waker>>,
}

#[repr(C)]
struct Schedulable<S: Schedule> {
    /// This must be the first field of the `Schedulable` struct!
    header: Header,
    scheduler: UnsafeCell<Option<S>>,
}

/// The current lifecycle stage of the future. Either the future itself or its output.
#[repr(C)] // https://github.com/rust-lang/miri/issues/3780
enum Stage<F: Future> {
    /// The future is still pending.
    Pending(F),

    /// The future has completed, and its output is ready to be taken by a
    /// `JoinHandle`, if one exists.
    Ready(Result<F::Output, JoinError<F::Output>>),

    /// The future has completed, and the task's output has been taken or is not
    /// needed.
    Consumed,
}

#[derive(Debug)]
pub(crate) struct Header {
    /// The task's state.
    ///
    /// This field is access with atomic instructions, so it's always safe to access it.
    state: State,
    /// The task vtable for this task.
    vtable: &'static VTable,
    /// The task's ID.
    id: Id,
    run_queue_links: mpsc_queue::Links<Self>,
    /// The tracing span associated with this task, for debugging purposes.
    span: tracing::Span,
    /// The TypeId of the scheduler that this task is associated with. Because the shape of a task
    /// depends on the scheduler type, a task MUST NOT be cross-shared between schedulers of different
    /// types (sharing between schedulers of the same type is fine and expected)
    #[cfg(debug_assertions)]
    scheduler_type: Option<TypeId>,
}

#[derive(Debug)]
struct VTable {
    /// Poll the future, returning a [`PollResult`] that indicates what the
    /// scheduler should do with the polled task.
    poll: unsafe fn(NonNull<Header>) -> PollResult,

    /// Poll the task's `JoinHandle` for completion, storing the output at the
    /// provided [`NonNull`] pointer if the task has completed.
    ///
    /// If the task has not completed, the [`Waker`] from the provided
    /// [`Context`] is registered to be woken when the task completes.
    // Splitting this up into type aliases just makes it *harder* to understand
    // IMO...
    #[expect(clippy::type_complexity, reason = "")]
    poll_join: unsafe fn(
        ptr: NonNull<Header>,
        outptr: NonNull<()>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), JoinError<()>>>,

    /// Drops the task and deallocates its memory.
    deallocate: unsafe fn(NonNull<Header>),

    /// The `wake_by_ref` function from the task's [`RawWakerVTable`].
    ///
    /// This is duplicated here as it's used to wake canceled tasks when a task
    /// is canceled by a [`TaskRef`] or [`JoinHandle`].
    wake_by_ref: unsafe fn(*const ()),
}

// === impl TaskRef ===

impl TaskRef {
    #[track_caller]
    pub(crate) fn new_allocated<S, F, A>(task: Box<Task<F, S>, A>) -> (Self, JoinHandle<F::Output>)
    where
        S: Schedule,
        F: Future,
        A: Allocator,
    {
        assert_eq!(task.state().refcount(), 1);
        let ptr = Box::into_raw(task);

        // Safety: we just allocated the ptr so it is never null
        let task = Self(unsafe { NonNull::new_unchecked(ptr).cast() });
        let join = JoinHandle::new(task.clone());

        (task, join)
    }

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

    /// Bind this task to a new scheduler
    ///
    /// # Safety
    ///
    /// The new scheduler `S` must be of the **same** type as the scheduler that this task got created
    /// with. The shape of the allocated tasks depend on the type of the scheduler, binding a task
    /// to a differently typed scheduler will therefore cause invalid memory accesses.
    pub(crate) unsafe fn bind_scheduler<S: Schedule + 'static>(&self, scheduler: S) {
        // Safety: ensured by caller
        unsafe {
            #[cfg(debug_assertions)]
            {
                if let Some(scheduler_type) = self.header().scheduler_type {
                    assert_eq!(
                        scheduler_type,
                        TypeId::of::<S>(),
                        "cannot bind {self:?} to a scheduler of type {}",
                        type_name::<S>(),
                    );
                }
            }

            self.0
                .cast::<Schedulable<S>>()
                .as_ref()
                .scheduler
                .with_mut(|current| *current = Some(scheduler));
        }
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
            task.is_stub=self.id().is_stub(),
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
            task.is_stub=self.id().is_stub(),
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

// === impl Task ===

impl<F: Future, S: Schedule> Task<F, S> {
    const TASK_VTABLE: VTable = VTable {
        poll: Self::poll,
        poll_join: Self::poll_join,
        deallocate: Self::deallocate,
        wake_by_ref: Schedulable::<S>::wake_by_ref,
    };

    loom_const_fn! {
        pub const fn new(future: F, task_id: Id, span: tracing::Span) -> Self {
            let inner = TaskInner {
                schedulable: Schedulable {
                    header: Header {
                        state: State::new(),
                        vtable: &Self::TASK_VTABLE,
                        id: task_id,
                        run_queue_links: mpsc_queue::Links::new(),
                        span,
                        #[cfg(debug_assertions)]
                        scheduler_type: Some(TypeId::of::<S>()),
                    },
                    scheduler: UnsafeCell::new(None),
                },
                stage: UnsafeCell::new(Stage::Pending(future)),
                join_waker: UnsafeCell::new(None),
            };
            Self(CachePadded(inner))
        }
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
                    this.0.0.join_waker.with_mut(|waker| {
                        waker.write(Some(cx.waker().clone()));
                    });
                }
                JoinAction::Reregister => {
                    this.0.0.join_waker.with_mut(|waker| {
                        let waker = (*waker).as_mut().unwrap();

                        let new_waker = cx.waker();
                        if !waker.will_wake(new_waker) {
                            *waker = new_waker.clone();
                        }
                    });
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
                task.is_stub=?this.as_ref().id().is_stub(),
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

        self.0.0.stage.with_mut(|stage| {
            // Safety: ensured by caller
            let stage = unsafe { &mut *stage };
            stage.poll(&mut cx, *self.id())
        })
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
            self.0.0.join_waker.with_mut(|waker| {
                if let Some(join_waker) = (*waker).take() {
                    tracing::trace!("waking {join_waker:?}");
                    join_waker.wake();
                } else {
                    tracing::trace!("called wake_join_waker on non-existing waker");
                }
            });
        }
    }

    unsafe fn take_output(&self, dst: NonNull<()>) {
        // Safety: ensured by caller
        unsafe {
            self.0.0.stage.with_mut(|stage| {
                match mem::replace(&mut *stage, Stage::Consumed) {
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
            });
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

impl Task<Stub, Stub> {
    const HEAP_STUB_VTABLE: VTable = VTable {
        poll: stub_poll,
        poll_join: stub_poll_join,
        // Heap allocated stub tasks *will* need to be deallocated, since the
        // scheduler will deallocate its stub task if it's dropped.
        deallocate: Self::deallocate,
        wake_by_ref: stub_wake_by_ref,
    };

    loom_const_fn! {
        /// Create a new stub task.
        pub(crate) const fn new_stub() -> Self {
            let inner = TaskInner {
                schedulable: Schedulable {
                    header: Header {
                        state: State::new(),
                        vtable: &Self::HEAP_STUB_VTABLE,
                        id: Id::stub(),
                        run_queue_links: mpsc_queue::Links::new_stub(),
                        span: tracing::Span::none(),
                        #[cfg(debug_assertions)]
                        scheduler_type: None,
                    },
                    scheduler: UnsafeCell::new(None),
                },
                stage: UnsafeCell::new(Stage::Pending(Stub)),
                join_waker: UnsafeCell::new(None),
            };

            Self(CachePadded(inner))
        }
    }
}

// === impl Stage ===

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

        cfg_if::cfg_if! {
            if #[cfg(test)] {
                let result = ::std::panic::catch_unwind(poll);
            } else if #[cfg(feature = "unwind2")] {
                let result = panic_unwind2::catch_unwind(poll);
            } else {
                let result = Ok(poll());
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

// === impl Schedulable ===

impl<S: Schedule> Schedulable<S> {
    const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_waker,
        Self::wake_by_val,
        Self::wake_by_ref,
        Self::drop_waker,
    );

    // `Waker::will_wake` is used all over the place to optimize waker code (e.g. only update wakers if they
    // have a different wake target). Problem is `will_wake` only checks for pointer equality and since
    // the `into_raw_waker` would usually be inlined in release mode (and with it `WAKER_VTABLE`) the
    // Waker identity would be different before and after calling `.clone()`. This isn't a correctness
    // problem since it's still the same waker in the end, it just causes a lot of unnecessary wake ups.
    // the `inline(never)` below is therefore quite load-bearing
    #[inline(never)]
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
                .with(|scheduler| {
                    (*scheduler)
                        .as_ref()
                        .expect("task doesn't have an associated scheduler, this is a bug!")
                        .wake(this);
                });
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

            let this = NonNull::new_unchecked(ptr.cast_mut());
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

            let this = NonNull::new_unchecked(this.cast_mut()).cast::<Self>();
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
            Self::drop_ref(NonNull::new_unchecked(this));
        }
    }
}

// === impl Header ===

// Safety: tasks are always treated as pinned in memory (a requirement for polling them)
// and care has been taken below to ensure the underlying memory isn't freed as long as the
// `TaskRef` is part of the owned tasks list.
unsafe impl mpsc_queue::Linked for Header {
    type Handle = TaskRef;

    fn into_ptr(task: Self::Handle) -> NonNull<Self> {
        let ptr = task.0;
        // converting a `TaskRef` into a pointer to enqueue it assigns ownership
        // of the ref count to the queue, so we don't want to run its `Drop`
        // impl.
        mem::forget(task);
        ptr
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        TaskRef(ptr)
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

/// DO NOT confuse this with [`TaskSTub`]. This type is just a zero-size placeholder so we
/// can plug *something* into the generics when creating the *heap allocated* stub task.
/// This type is *not* publicly exported, contrary to [`TaskSTub`] which users will have to statically
/// allocate themselves.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Stub;

impl Future for Stub {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        unreachable!("the stub task should never be polled!")
    }
}

impl Schedule for Stub {
    fn tick_n(&self, _n: usize) -> Tick {
        unimplemented!("stub scheduler should never be called!");
    }
    fn try_steal(&self) -> Result<Stealer<Self>, TryStealError> {
        unimplemented!("stub scheduler should never be called!");
    }
    fn current_task(&self) -> Option<TaskRef> {
        unimplemented!("stub scheduler should never be called!");
    }
    fn wake(&self, _: TaskRef) {
        unimplemented!("stub scheduler should never be called!");
    }
    fn spawn(&self, _: TaskRef) {
        unimplemented!("stub scheduler should never be called!");
    }
}

/// A stub task required by many `const` constructors in this crate. You should rarely need to use
/// this directly, instead look for the safe construction macros provided.
#[derive(Debug)]
pub struct TaskStub {
    pub(crate) header: Header,
}

impl Default for TaskStub {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskStub {
    const STATIC_STUB_VTABLE: VTable = VTable {
        poll: stub_poll,
        poll_join: stub_poll_join,
        deallocate: stub_deallocate,
        wake_by_ref: stub_wake_by_ref,
    };

    loom_const_fn! {
        pub const fn new() -> Self {
            Self {
                header: Header {
                    state: State::new(),
                    vtable: &Self::STATIC_STUB_VTABLE,
                    id: Id::stub(),
                    run_queue_links: mpsc_queue::Links::new_stub(),
                    span: tracing::Span::none(),
                    #[cfg(debug_assertions)]
                    scheduler_type: None
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
unsafe fn stub_poll(ptr: NonNull<Header>) -> PollResult {
    // Safety: this method should never be called
    unsafe {
        debug_assert!(ptr.as_ref().id.is_stub());
        unreachable!("stub task ({ptr:?}) should never be polled!");
    }
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
unsafe fn stub_deallocate(ptr: NonNull<Header>) {
    // Safety: this method should never be called
    unsafe {
        debug_assert!(ptr.as_ref().id.is_stub());
        unreachable!("stub task ({ptr:p}) should never be deallocated!");
    }
}

#[unsafe(no_mangle)]
unsafe fn stub_wake_by_ref(ptr: *const ()) {
    unreachable!("stub task ({ptr:p}) has no waker and should never be woken!");
}
