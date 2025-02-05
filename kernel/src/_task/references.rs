// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::panic;
use crate::task::error::JoinError;
use crate::task::id::Id;
use crate::task::raw::{
    get_id_offset, get_trailer_offset, Core, Header, Stage, Task, Trailer, Vtable,
};
use crate::task::state::{
    Snapshot, State, TransitionToIdle, TransitionToNotifiedByRef, TransitionToNotifiedByVal,
    TransitionToRunning,
};
use crate::task::waker::waker_ref;
use crate::task::Schedule;
use alloc::boxed::Box;
use core::alloc::Layout;
use core::any::Any;
use core::cell::UnsafeCell;
use core::future::Future;
use core::mem;
use core::mem::ManuallyDrop;
use core::panic::AssertUnwindSafe;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};

/// A type-erased, reference-counted pointer to a spawned [`Task`].
///
/// `TaskRef`s are reference-counted, and the task will be deallocated when the
/// last `TaskRef` pointing to it is dropped.
#[derive(Debug, Eq, PartialEq)]
pub struct TaskRef(NonNull<Header>);

/// A typed pointer to a spawned [`Task`]. It's roughly a lower-level version of [`TaskRef`]
/// that is not reference counted and tied to a specific tasks future type and scheduler.
struct RawTaskRef<F: Future, S> {
    ptr: NonNull<Task<F, S>>,
}

impl TaskRef {
    pub(crate) fn new_stub() -> Self {
        Self(RawTaskRef::new_stub().ptr.cast())
    }

    #[expect(tail_expr_drop_order, reason = "")]
    pub(crate) fn new<F, S>(future: F, scheduler: S, task_id: Id) -> (Self, Self, Self)
    where
        F: Future,
        S: Schedule + 'static,
    {
        let ptr = RawTaskRef::new(future, scheduler, task_id).ptr.cast();
        (Self(ptr), Self(ptr), Self(ptr))
    }

    pub(crate) fn clone_from_raw(ptr: NonNull<Header>) -> Self {
        let this = Self(ptr);
        this.state().ref_inc();
        this
    }

    pub(crate) unsafe fn from_raw(ptr: NonNull<Header>) -> Self {
        Self(ptr)
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

    pub(crate) fn run(self) {
        self.poll();
        mem::forget(self);
    }

    pub(crate) fn poll(&self) {
        let vtable = self.header().vtable;
        // Safety: constructor ensures the pointer is always valid
        unsafe {
            (vtable.poll)(self.0);
        }
    }
    pub(crate) fn schedule(&self) {
        let vtable = self.header().vtable;
        // Safety: constructor ensures the pointer is always valid
        unsafe {
            (vtable.schedule)(self.0);
        }
    }
    pub(crate) fn dealloc(&self) {
        let vtable = self.header().vtable;
        // Safety: constructor ensures the pointer is always valid
        unsafe {
            (vtable.dealloc)(self.0);
        }
    }
    pub(crate) unsafe fn try_read_output(&self, dst: *mut (), waker: &Waker) {
        let vtable = self.header().vtable;
        // Safety: constructor ensures the pointer is always valid
        unsafe {
            (vtable.try_read_output)(self.0, dst, waker);
        }
    }
    pub(crate) fn drop_join_handle_slow(&self) {
        let vtable = self.header().vtable;
        // Safety: constructor ensures the pointer is always valid
        unsafe { (vtable.drop_join_handle_slow)(self.0) }
    }
    pub(crate) fn shutdown(&self) {
        let vtable = self.header().vtable;
        // Safety: constructor ensures the pointer is always valid
        unsafe { (vtable.shutdown)(self.0) }
    }
    pub(crate) fn drop_reference(&self) {
        if self.state().ref_dec() {
            self.dealloc();
        }
    }
    /// This call consumes a ref-count and notifies the task. This will create a
    /// new Notified and submit it if necessary.
    ///
    /// The caller does not need to hold a ref-count besides the one that was
    /// passed to this call.
    pub(crate) fn wake_by_val(&self) {
        match self.state().transition_to_notified_by_val() {
            TransitionToNotifiedByVal::Submit => {
                debug_assert!(self.state().load().ref_count() >= 2);
                // The caller has given us a ref-count, and the transition has
                // created a new ref-count, so we now hold two. We turn the new
                // ref-count Notified and pass it to the call to `schedule`.
                //
                // The old ref-count is retained for now to ensure that the task
                // is not dropped during the call to `schedule` if the call
                // drops the task it was given.
                self.schedule();

                // Now that we have completed the call to schedule, we can
                // release our ref-count.
                self.drop_reference();
            }
            TransitionToNotifiedByVal::Dealloc => {
                self.dealloc();
            }
            TransitionToNotifiedByVal::DoNothing => {}
        }
    }

    /// This call notifies the task. It will not consume any ref-counts, but the
    /// caller should hold a ref-count.  This will create a new Notified and
    /// submit it if necessary.
    pub(crate) fn wake_by_ref(&self) {
        match self.state().transition_to_notified_by_ref() {
            TransitionToNotifiedByRef::Submit => {
                // The transition above incremented the ref-count for a new task
                // and the caller also holds a ref-count. The caller's ref-count
                // ensures that the task is not destroyed even if the new task
                // is dropped before `schedule` returns.
                self.schedule();
            }
            TransitionToNotifiedByRef::DoNothing => {}
        }
    }

    /// Remotely aborts the task.
    ///
    /// The caller should hold a ref-count, but we do not consume it.
    ///
    /// This is similar to `shutdown` except that it asks the runtime to perform
    /// the shutdown. This is necessary to avoid the shutdown happening in the
    /// wrong thread for non-Send tasks.
    pub(crate) fn remote_abort(&self) {
        if self.state().transition_to_notified_and_cancel() {
            // The transition has created a new ref-count, which we turn into
            // a Notified and pass to the task.
            //
            // Since the caller holds a ref-count, the task cannot be destroyed
            // before the call to `schedule` returns even if the call drops the
            // `Notified` internally.
            self.schedule();
        }
    }
}

impl Clone for TaskRef {
    #[inline]
    #[track_caller]
    fn clone(&self) -> Self {
        log::trace!("TaskRef::clone {:?}", self.0);
        self.state().ref_inc();
        Self(self.0)
    }
}

impl Drop for TaskRef {
    #[inline]
    #[track_caller]
    fn drop(&mut self) {
        // log::trace!("TaskRef::drop {:?}", self.0);
        if self.state().ref_dec() {
            self.dealloc();
        }
    }
}

// Safety: task refs are "just" atomically reference counted pointers and the state lifecycle system ensures mutual
// exclusion for mutating methods, thus this type is always Send
unsafe impl Send for TaskRef {}
// Safety: task refs are "just" atomically reference counted pointers and the state lifecycle system ensures mutual
// exclusion for mutating methods, thus this type is always Sync
unsafe impl Sync for TaskRef {}

impl<F, S> RawTaskRef<F, S>
where
    F: Future,
    S: Schedule + 'static,
{
    const TASK_VTABLE: Vtable = Vtable {
        poll: Self::poll,
        schedule: Self::schedule,
        dealloc: Self::dealloc,
        try_read_output: Self::try_read_output,
        drop_join_handle_slow: Self::drop_join_handle_slow,
        shutdown: Self::shutdown,
        id_offset: get_id_offset::<F, S>(),
        trailer_offset: get_trailer_offset::<F, S>(),
    };

    pub fn new(future: F, scheduler: S, task_id: Id) -> Self {
        let ptr = Box::into_raw(Box::new(Task {
            header: Header {
                state: State::new(),
                vtable: &Self::TASK_VTABLE,
            },
            core: Core {
                scheduler,
                stage: UnsafeCell::new(Stage::Running(future)),
                task_id,
            },
            trailer: Trailer {
                waker: UnsafeCell::new(None),
                run_queue_links: mpsc_queue::Links::default(),
                owned_tasks_links: linked_list::Links::default(),
            },
        }));

        log::trace!(
            "allocated task ptr {ptr:?} with layout {:?}",
            Layout::new::<Task<F, S>>()
        );
        Self {
            // Safety: we just allocated the pointer, it is always valid
            ptr: unsafe { NonNull::new_unchecked(ptr) },
        }
    }

    unsafe fn poll(ptr: NonNull<Header>) {
        log::trace!("RawTaskRef::poll {ptr:p}");

        // Safety: this method gets called through the vtable ensuring that the pointer is valid
        // for this `RawTaskRef`'s `F` and `S` generics.
        unsafe {
            let this = Self::from_raw(ptr);

            // We pass our ref-count to `poll_inner`.
            match Self::poll_inner(ptr) {
                PollResult::Notified => {
                    debug_assert!(this.state().load().ref_count() >= 2);
                    // The `poll_inner` call has given us two ref-counts back.
                    // We give one of them to a new task and call `yield_now`.
                    this.core().scheduler.yield_now(this.get_new_task());

                    // The remaining ref-count is now dropped. We kept the extra
                    // ref-count until now to ensure that even if the `yield_now`
                    // call drops the provided task, the task isn't deallocated
                    // before after `yield_now` returns.
                    this.drop_reference();
                }
                PollResult::Complete => {
                    this.complete();
                }
                PollResult::Dealloc => {
                    Self::dealloc(ptr);
                }
                PollResult::Done => (),
            }
        }
    }

    unsafe fn poll_inner(ptr: NonNull<Header>) -> PollResult {
        // Safety: caller has to ensure `ptr` is valid
        let this = unsafe { Self::from_raw(ptr) };

        match this.state().transition_to_running() {
            TransitionToRunning::Success => {
                // Separated to reduce LLVM codegen
                fn transition_result_to_poll_result(result: TransitionToIdle) -> PollResult {
                    match result {
                        TransitionToIdle::Ok => PollResult::Done,
                        TransitionToIdle::OkNotified => PollResult::Notified,
                        TransitionToIdle::OkDealloc => PollResult::Dealloc,
                        TransitionToIdle::Cancelled => PollResult::Complete,
                    }
                }
                let header_ptr = this.header_ptr();
                let waker_ref = waker_ref::<S>(&header_ptr);
                let cx = Context::from_waker(&waker_ref);
                // Safety: `transition_to_running` returns `Success` only when we have exclusive
                // access
                let res = unsafe { poll_future(this.core(), cx) };

                if res == Poll::Ready(()) {
                    // The future completed. Move on to complete the task.
                    return PollResult::Complete;
                }

                let transition_res = this.state().transition_to_idle();
                if let TransitionToIdle::Cancelled = transition_res {
                    // The transition to idle failed because the task was
                    // cancelled during the poll.
                    // Safety: `transition_to_running` returns `Success` only when we have exclusive
                    // access
                    unsafe { cancel_task(this.core()) };
                }
                transition_result_to_poll_result(transition_res)
            }
            TransitionToRunning::Cancelled => {
                // Safety: `transition_to_running` returns `Cancelled` only when we have exclusive
                // access
                unsafe { cancel_task(this.core()) };
                PollResult::Complete
            }
            TransitionToRunning::Failed => PollResult::Done,
            TransitionToRunning::Dealloc => PollResult::Dealloc,
        }
    }

    unsafe fn schedule(ptr: NonNull<Header>) {
        log::trace!("RawTaskRef::schedule {ptr:p}");

        // Safety: this method gets called through the vtable ensuring that the pointer is valid
        // for this `RawTaskRef`'s `F` and `S` generics.
        unsafe {
            let this = Self::from_raw(ptr);
            this.core().scheduler.schedule(this.get_new_task());
        }
    }

    unsafe fn dealloc(ptr: NonNull<Header>) {
        log::trace!("RawTaskRef::dealloc {ptr:p}");

        // Safety: The caller of this method just transitioned our ref-count to
        // zero, so it is our responsibility to release the allocation.
        //
        // We don't hold any references into the allocation at this point, but
        // it is possible for another thread to still hold a `&State` into the
        // allocation if that other thread has decremented its last ref-count,
        // but has not yet returned from the relevant method on `State`.
        //
        // However, the `State` type consists of just an `AtomicUsize`, and an
        // `AtomicUsize` wraps the entirety of its contents in an `UnsafeCell`.
        // As explained in the documentation for `UnsafeCell`, such references
        // are allowed to be dangling after their last use, even if the
        // reference has not yet gone out of scope.
        //
        // Additionally, this method gets called through the vtable ensuring that
        // the pointer is valid for this `RawTaskRef`'s `F` and `S` generics.
        unsafe {
            debug_assert_eq!(ptr.as_ref().state.load().ref_count(), 0);
            drop(Box::from_raw(ptr.cast::<Task<F, S>>().as_ptr()));
        }
    }

    unsafe fn try_read_output(ptr: NonNull<Header>, dst: *mut (), waker: &Waker) {
        log::trace!("RawTaskRef::try_read_output {ptr:p}");

        // Safety: this method gets called through the vtable ensuring that the pointer is valid
        // for this `RawTaskRef`'s `F` and `S` generics. The caller has to ensure the `dst` pointer
        // is valid.
        unsafe {
            let this = Self::from_raw(ptr);
            let dst = dst.cast::<Poll<super::Result<F::Output>>>();
            if can_read_output(this.header(), this.trailer(), waker) {
                *dst = Poll::Ready(this.core().take_output());
            }
        }
    }

    unsafe fn drop_join_handle_slow(ptr: NonNull<Header>) {
        log::trace!("RawTaskRef::drop_join_handle_slow {ptr:p}");

        // Safety: this method gets called through the vtable ensuring that the pointer is valid
        // for this `RawTaskRef`'s `F` and `S` generics
        unsafe {
            let this = Self::from_raw(ptr);

            // Try to unset `JOIN_INTEREST` and `JOIN_WAKER`. This must be done as a first step in
            // case the task concurrently completed.
            let transition = this.state().transition_to_join_handle_dropped();

            if transition.drop_output {
                // It is our responsibility to drop the output. This is critical as
                // the task output may not be `Send` and as such must remain with
                // the scheduler or `JoinHandle`. i.e. if the output remains in the
                // task structure until the task is deallocated, it may be dropped
                // by a Waker on any arbitrary thread.
                //
                // Panics are delivered to the user via the `JoinHandle`. Given that
                // they are dropping the `JoinHandle`, we assume they are not
                // interested in the panic and swallow it.
                let _ = panic::catch_unwind(AssertUnwindSafe(|| {
                    this.core().drop_future_or_output();
                }));
            }

            if transition.drop_waker {
                // If the JOIN_WAKER flag is unset at this point, the task is either
                // already terminal or not complete so the `JoinHandle` is responsible
                // for dropping the waker.
                // Safety:
                // If the JOIN_WAKER bit is not set the join handle has exclusive
                // access to the waker as per rule 2 in task/mod.rs.
                // This can only be the case at this point in two scenarios:
                // 1. The task completed and the runtime unset `JOIN_WAKER` flag
                //    after accessing the waker during task completion. So the
                //    `JoinHandle` is the only one to access the  join waker here.
                // 2. The task is not completed so the `JoinHandle` was able to unset
                //    `JOIN_WAKER` bit itself to get mutable access to the waker.
                //    The runtime will not access the waker when this flag is unset.
                this.trailer().set_waker(None);
            }

            // Drop the `JoinHandle` reference, possibly deallocating the task
            this.drop_reference();
        }
    }

    unsafe fn shutdown(ptr: NonNull<Header>) {
        log::trace!("RawTaskRef::shutdown {ptr:p}");

        // Safety: this method gets called through the vtable ensuring that the pointer is valid
        // for this `RawTaskRef`'s `F` and `S` generics
        unsafe {
            let this = Self::from_raw(ptr);

            if !this.state().transition_to_shutdown() {
                // The task is concurrently running. No further work needed.
                this.drop_reference();
                return;
            }

            // By transitioning the lifecycle to `Running`, we have permission to
            // drop the future.
            cancel_task(this.core());
            this.complete();
        }
    }

    /// Construct a typed task reference from an untyped pointer to a task.
    ///
    /// # Safety
    ///
    /// The caller has to ensure `ptr` is a valid task AND that the tasks output and scheduler
    /// match this types generic arguments `F` and `S`. Getting this wrong e.g. calling
    /// `RawTaskRef::<(), S>::from_raw` on a task that has the output type `i32` will likely lead
    /// to stack corruption.
    unsafe fn from_raw(ptr: NonNull<Header>) -> Self {
        Self { ptr: ptr.cast() }
    }

    fn header_ptr(&self) -> NonNull<Header> {
        self.ptr.cast()
    }

    fn header(&self) -> &Header {
        // Safety: constructor ensures the pointer is always valid
        unsafe { &*self.header_ptr().as_ptr() }
    }

    fn state(&self) -> &State {
        &self.header().state
    }

    fn core(&self) -> &Core<F, S> {
        // Safety: constructor ensures the pointer is always valid
        unsafe { &self.ptr.as_ref().core }
    }

    fn trailer(&self) -> &Trailer {
        // Safety: constructor ensures the pointer is always valid
        unsafe { &self.ptr.as_ref().trailer }
    }

    fn complete(&self) {
        // The future has completed and its output has been written to the task
        // stage. We transition from running to complete.
        let snapshot = self.state().transition_to_complete();

        // We catch panics here in case dropping the future or waking the
        // JoinHandle panics.
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            if !snapshot.is_join_interested() {
                // The `JoinHandle` is not interested in the output of
                // this task. It is our responsibility to drop the
                // output. The join waker was already dropped by the
                // `JoinHandle` before.
                // Safety: the COMPLETE bit has been set above and the JOIN_INTEREST bit is unset
                // so according to rule 7 we have mutable exclusive access
                unsafe {
                    self.core().drop_future_or_output();
                }
            } else if snapshot.is_join_waker_set() {
                // Notify the waker.
                // Safety: Reading the waker field is safe per rule 4
                // in task/mod.rs, since the JOIN_WAKER bit is set and the call
                // to transition_to_complete() above set the COMPLETE bit.
                unsafe {
                    self.trailer().wake_join();
                }

                // Inform the `JoinHandle` that we are done waking the waker by
                // unsetting the `JOIN_WAKER` bit. If the `JoinHandle` has
                // already been dropped and `JOIN_INTEREST` is unset, then we must
                // drop the waker ourselves.
                if !self
                    .state()
                    .unset_waker_after_complete()
                    .is_join_interested()
                {
                    // SAFETY: We have COMPLETE=1 and JOIN_INTEREST=0, so
                    // we have exclusive access to the waker.
                    unsafe { self.trailer().set_waker(None) };
                }
            }
        }));

        // The task has completed execution and will no longer be scheduled.
        let num_release = self.release();

        if self.state().transition_to_terminal(num_release) {
            // Safety: `ref_dec` returns true if no other references exist, so deallocation is safe
            unsafe {
                Self::dealloc(self.ptr.cast());
            }
        }
    }

    fn drop_reference(self) {
        if self.state().ref_dec() {
            // Safety: `ref_dec` returns true if no other references exist, so deallocation is safe
            unsafe {
                Self::dealloc(self.ptr.cast());
            }
        }
    }

    fn get_new_task(&self) -> TaskRef {
        // safety: The header is at the beginning of the cell, so this cast is
        // safe.
        unsafe { TaskRef::from_raw(self.ptr.cast()) }
    }

    /// Releases the task from the scheduler. Returns the number of ref-counts
    /// that should be decremented.
    fn release(&self) -> usize {
        // We don't actually increment the ref-count here, but the new task is
        // never destroyed, so that's ok.
        let me = ManuallyDrop::new(self.get_new_task());

        if let Some(task) = self.core().scheduler.release(&me) {
            mem::forget(task);
        }

        1
    }
}

struct Stub;
impl Future for Stub {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unreachable!("poll called on a stub future")
    }
}

impl Schedule for Stub {
    fn schedule(&self, _task: TaskRef) {
        unreachable!("schedule called on a stub scheduler")
    }
    fn release(&self, _task: &TaskRef) -> Option<TaskRef> {
        unreachable!("release called on a stub scheduler")
    }
    fn yield_now(&self, _task: TaskRef) {
        unreachable!("yield_now called on a stub scheduler")
    }
}

impl RawTaskRef<Stub, Stub> {
    const STUB_VTABLE: Vtable = Vtable {
        poll: Self::poll_stub,
        schedule: Self::schedule_stub,
        dealloc: Self::dealloc,
        try_read_output: Self::try_read_output_stub,
        drop_join_handle_slow: Self::drop_join_handle_slow_stub,
        shutdown: Self::shutdown_stub,
        id_offset: get_id_offset::<Stub, Stub>(),
        trailer_offset: get_trailer_offset::<Stub, Stub>(),
    };

    pub fn new_stub() -> Self {
        let ptr = Box::into_raw(Box::new(Task {
            header: Header {
                state: State::new(),
                vtable: &Self::STUB_VTABLE,
            },
            core: Core {
                scheduler: Stub,
                stage: UnsafeCell::new(Stage::Running(Stub)),
                task_id: Id::stub(),
            },
            trailer: Trailer {
                waker: UnsafeCell::new(None),
                run_queue_links: mpsc_queue::Links::default(),
                owned_tasks_links: linked_list::Links::default(),
            },
        }));
        log::trace!("allocated stub ptr {ptr:?}");

        Self {
            // Safety: we just allocated the pointer, it is always valid
            ptr: unsafe { NonNull::new_unchecked(ptr) },
        }
    }

    unsafe fn poll_stub(_ptr: NonNull<Header>) {
        // Safety: this method should never be called
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("poll_stub called on a stub task");
        }
    }

    unsafe fn schedule_stub(_ptr: NonNull<Header>) {
        // Safety: this method should never be called
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("schedule_stub called on a stub task");
        }
    }

    unsafe fn try_read_output_stub(_ptr: NonNull<Header>, _dst: *mut (), _waker: &Waker) {
        // Safety: this method should never be called
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("try_read_output_stub called on a stub task");
        }
    }

    unsafe fn drop_join_handle_slow_stub(_ptr: NonNull<Header>) {
        // Safety: this method should never be called
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("drop_join_handle_slow_stub called on a stub task");
        }
    }

    /// # Safety
    ///
    /// The caller must ensure the pointer is valid
    unsafe fn shutdown_stub(_ptr: NonNull<Header>) {
        // Safety: this method should never be called
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("shutdown_stub called on a stub task");
        }
    }
}

#[expect(tail_expr_drop_order, reason = "TODO")]
fn can_read_output(header: &Header, trailer: &Trailer, waker: &Waker) -> bool {
    // Load a snapshot of the current task state
    let snapshot = header.state.load();

    debug_assert!(snapshot.is_join_interested());

    if !snapshot.is_complete() {
        // If the task is not complete, try storing the provided waker in the
        // task's waker field.

        let res = if snapshot.is_join_waker_set() {
            // If JOIN_WAKER is set, then JoinHandle has previously stored a
            // waker in the waker field per step (iii) of rule 5 in task/mod.rs.

            // Optimization: if the stored waker and the provided waker wake the
            // same task, then return without touching the waker field.
            // Safety: Reading the waker field below is safe per rule 3 in task/mod.rs.
            if unsafe { trailer.will_wake(waker) } {
                return false;
            }

            // Otherwise swap the stored waker with the provided waker by
            // following the rule 5 in task/mod.rs.
            header
                .state
                .unset_waker()
                .and_then(|snapshot| set_join_waker(header, trailer, waker.clone(), snapshot))
        } else {
            // If JOIN_WAKER is unset, then JoinHandle has mutable access to the
            // waker field per rule 2 in task/mod.rs; therefore, skip step (i)
            // of rule 5 and try to store the provided waker in the waker field.
            // Safety: absence of JOIN_WAKER means we have exclusive access
            set_join_waker(header, trailer, waker.clone(), snapshot)
        };

        match res {
            Ok(_) => return false,
            Err(snapshot) => {
                assert!(snapshot.is_complete());
            }
        }
    }
    true
}

fn set_join_waker(
    header: &Header,
    trailer: &Trailer,
    waker: Waker,
    snapshot: Snapshot,
) -> Result<Snapshot, Snapshot> {
    assert!(snapshot.is_join_interested());
    assert!(!snapshot.is_join_waker_set());

    // Safety: Only the `JoinHandle` may set the `waker` field. When
    // `JOIN_INTEREST` is **not** set, nothing else will touch the field.
    unsafe {
        trailer.set_waker(Some(waker));

        // Update the `JoinWaker` state accordingly
        let res = header.state.set_join_waker();

        // If the state could not be updated, then clear the join waker
        if res.is_err() {
            trailer.set_waker(None);
        }

        res
    }
}

pub enum PollResult {
    Complete,
    Notified,
    Done,
    Dealloc,
}

/// Cancels the task and store the appropriate error in the stage field.
///
/// # Safety
///
/// The caller has to ensure this hart has exclusive mutable access to the tasks `stage` field (ie the
/// future or output).
unsafe fn cancel_task<T: Future, S>(core: &Core<T, S>) {
    // Safety: caller has to ensure mutual exclusion
    unsafe {
        // Drop the future from a panic guard.
        let res = panic::catch_unwind(AssertUnwindSafe(|| {
            core.drop_future_or_output();
        }));

        core.store_output(Err(panic_result_to_join_error(core.task_id, res)));
    }
}

/// Polls the future. If the future completes, the output is written to the
/// stage field.
///
/// # Safety
///
/// The caller has to ensure this hart has exclusive mutable access to the tasks `stage` field (ie the
/// future or output).
unsafe fn poll_future<T: Future, S: Schedule>(core: &Core<T, S>, cx: Context<'_>) -> Poll<()> {
    // Poll the future.
    let output = panic::catch_unwind(AssertUnwindSafe(|| {
        struct Guard<'a, T: Future, S: Schedule> {
            core: &'a Core<T, S>,
        }
        impl<T: Future, S: Schedule> Drop for Guard<'_, T, S> {
            fn drop(&mut self) {
                // If the future panics on poll, we drop it inside the panic
                // guard.
                // Safety: caller has to ensure mutual exclusion
                unsafe {
                    self.core.drop_future_or_output();
                }
            }
        }
        let guard = Guard { core };
        // Safety: caller has to ensure mutual exclusion
        let res = unsafe { guard.core.poll(cx) };
        mem::forget(guard);
        res
    }));

    // Prepare output for being placed in the core stage.
    let output = match output {
        Ok(Poll::Pending) => return Poll::Pending,
        Ok(Poll::Ready(output)) => Ok(output),
        Err(panic) => Err(panic_to_error(core.task_id, panic)),
    };

    // Catch and ignore panics if the future panics on drop.
    // Safety: caller has to ensure mutual exclusion
    let res = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        core.store_output(output);
    }));

    assert!(res.is_ok(), "unhandled panic {res:?}");

    Poll::Ready(())
}

fn panic_result_to_join_error(
    task_id: Id,
    res: Result<(), Box<dyn Any + Send + 'static>>,
) -> JoinError {
    match res {
        Ok(()) => JoinError::cancelled(task_id),
        Err(panic) => JoinError::panic(task_id, panic),
    }
}

#[cold]
fn panic_to_error(task_id: Id, panic: Box<dyn Any + Send + 'static>) -> JoinError {
    log::error!("unhandled panic");
    // scheduler().unhandled_panic();
    JoinError::panic(task_id, panic)
}
