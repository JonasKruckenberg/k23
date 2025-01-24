// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::panic;
use super::error::JoinError;
use super::id::Id;
use super::state::{
    Snapshot, State, TransitionToIdle, TransitionToNotifiedByRef, TransitionToNotifiedByVal,
    TransitionToRunning,
};
use super::waker::waker_ref;
use super::{PollResult, Schedule};
use alloc::boxed::Box;
use core::alloc::Layout;
use core::any::Any;
use core::cell::UnsafeCell;
use core::future::Future;
use core::marker::PhantomData;
use core::mem;
use core::mem::{offset_of, ManuallyDrop};
use core::panic::AssertUnwindSafe;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};

/// A type-erased, reference-counted pointer to a spawned [`Task`].
///
/// `TaskRef`s are reference-counted, and the task will be deallocated when the
/// last `TaskRef` pointing to it is dropped.
#[derive(Eq, PartialEq)]
pub struct TaskRef(NonNull<Header>);

/// A non-Send variant of Notified with the invariant that it is on a thread
/// where it is safe to poll it.
#[repr(transparent)]
pub struct LocalTaskRef {
    pub(super) task: TaskRef,
    pub(super) _not_send: PhantomData<*const ()>,
}

/// A typed pointer to a spawned [`Task`]. It's roughly a lower-level version of [`TaskRef`]
/// that is not reference counted and tied to a specific tasks future type and scheduler.
struct RawTaskRef<F: Future, S> {
    ptr: NonNull<Task<F, S>>,
}

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
/// [scheduler]: crate::scheduler::Schedule
/// [task storage]: Storage
/// [dynamic dispatch]: https://en.wikipedia.org/wiki/Dynamic_dispatch
// # This struct should be cache padded to avoid false sharing. The cache padding rules are copied
// from crossbeam-utils/src/cache_padded.rs
//
// Starting from Intel's Sandy Bridge, spatial prefetcher is now pulling pairs of 64-byte cache
// lines at a time, so we have to align to 128 bytes rather than 64.
//
// Sources:
// - https://www.intel.com/content/dam/www/public/us/en/documents/manuals/64-ia-32-architectures-optimization-manual.pdf
// - https://github.com/facebook/folly/blob/1b5288e6eea6df074758f877c849b6e73bbb9fbb/folly/lang/Align.h#L107
//
// ARM's big.LITTLE architecture has asymmetric cores and "big" cores have 128-byte cache line size.
//
// Sources:
// - https://www.mono-project.com/news/2016/09/12/arm64-icache/
//
// powerpc64 has 128-byte cache line size.
//
// Sources:
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_ppc64x.go#L9
#[cfg_attr(
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64",
    ),
    repr(align(128))
)]
// arm, mips, mips64, sparc, and hexagon have 32-byte cache line size.
//
// Sources:
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_arm.go#L7
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_mips.go#L7
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_mipsle.go#L7
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_mips64x.go#L9
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/sparc/include/asm/cache.h#L17
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/hexagon/include/asm/cache.h#L12
#[cfg_attr(
    any(
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips64",
        target_arch = "sparc",
        target_arch = "hexagon",
    ),
    repr(align(32))
)]
// m68k has 16-byte cache line size.
//
// Sources:
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/m68k/include/asm/cache.h#L9
#[cfg_attr(target_arch = "m68k", repr(align(16)))]
// s390x has 256-byte cache line size.
//
// Sources:
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_s390x.go#L7
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/s390/include/asm/cache.h#L13
#[cfg_attr(target_arch = "s390x", repr(align(256)))]
// x86, riscv, wasm, and sparc64 have 64-byte cache line size.
//
// Sources:
// - https://github.com/golang/go/blob/dda2991c2ea0c5914714469c4defc2562a907230/src/internal/cpu/cpu_x86.go#L9
// - https://github.com/golang/go/blob/3dd58676054223962cd915bb0934d1f9f489d4d2/src/internal/cpu/cpu_wasm.go#L7
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/sparc/include/asm/cache.h#L19
// - https://github.com/torvalds/linux/blob/3516bd729358a2a9b090c1905bd2a3fa926e24c6/arch/riscv/include/asm/cache.h#L10
//
// All others are assumed to have 64-byte cache line size.
#[cfg_attr(
    not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64",
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips64",
        target_arch = "sparc",
        target_arch = "hexagon",
        target_arch = "m68k",
        target_arch = "s390x",
    )),
    repr(align(64))
)]
#[repr(C)]
struct Task<F: Future, S> {
    header: Header,
    core: Core<F, S>,
    trailer: Trailer,
}

#[repr(C)]
#[derive(Debug)]
pub struct Header {
    /// Task state which can be atomically updated.
    pub(super) state: State,
    /// The task vtable for this task.
    ///
    /// Note that this is different from the [waker vtable], which contains
    /// pointers to the waker methods (and depends primarily on the task's
    /// scheduler type). The task vtable instead contains methods for
    /// interacting with the task's future, such as polling it and reading the
    /// task's output. These depend primarily on the type of the future rather
    /// than the scheduler.
    ///
    /// [waker vtable]: core::task::RawWakerVTable
    pub(super) vtable: &'static Vtable,
}

#[repr(C)]
#[derive(Debug)]
pub struct Core<F: Future, S> {
    pub(super) scheduler: S,
    /// Either the future or the output.
    stage: UnsafeCell<Stage<F>>,
    /// The task's ID, used for populating `JoinError`s.
    pub(super) task_id: Id,
}

/// Either the future or the output.
#[repr(C)] // https://github.com/rust-lang/miri/issues/3780
pub(super) enum Stage<T: Future> {
    Running(T),
    Finished(super::Result<T::Output>),
    Consumed,
}

#[repr(C)]
#[derive(Debug)]
pub struct Trailer {
    /// Consumer task waiting on completion of this task.
    waker: UnsafeCell<Option<Waker>>,
    run_queue_links: mpsc_queue::Links<Header>,
    owned_tasks_links: linked_list::Links<Header>,
}

#[derive(Debug)]
pub(super) struct Vtable {
    /// Polls the future.
    pub(super) poll: unsafe fn(NonNull<Header>),
    /// Schedules the task for execution on the runtime.
    schedule: unsafe fn(NonNull<Header>),
    /// Deallocates the memory.
    pub(super) dealloc: unsafe fn(NonNull<Header>),
    /// Reads the task output, if complete.
    pub(super) try_read_output: unsafe fn(NonNull<Header>, *mut (), &Waker),
    /// The join handle has been dropped.
    pub(super) drop_join_handle_slow: unsafe fn(NonNull<Header>),
    /// Scheduler is being shutdown.
    pub(super) shutdown: unsafe fn(NonNull<Header>),
    /// The number of bytes that the `id` field is offset from the header.
    id_offset: usize,
    /// The number of bytes that the `trailer` field is offset from the header.
    trailer_offset: usize,
}

impl TaskRef {
    pub(crate) fn new_stub() -> Self {
        Self(RawTaskRef::new_stub().ptr.cast())
    }

    #[allow(tail_expr_drop_order)]
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

    pub(super) unsafe fn from_raw(ptr: NonNull<Header>) -> Self {
        Self(ptr)
    }

    pub(in crate::async_rt) fn header_ptr(&self) -> NonNull<Header> {
        self.0
    }
    pub(super) fn header(&self) -> &Header {
        unsafe { self.0.as_ref() }
    }
    /// Returns a reference to the task's state.
    pub(super) fn state(&self) -> &State {
        &self.header().state
    }

    pub(in crate::async_rt) fn run(self) {
        self.poll();
        mem::forget(self);
    }

    pub(in crate::async_rt) fn poll(&self) {
        let vtable = self.header().vtable;
        unsafe {
            (vtable.poll)(self.0);
        }
    }
    pub(super) fn schedule(&self) {
        let vtable = self.header().vtable;
        unsafe {
            (vtable.schedule)(self.0);
        }
    }
    pub(super) fn dealloc(&self) {
        let vtable = self.header().vtable;
        unsafe {
            (vtable.dealloc)(self.0);
        }
    }
    pub(super) unsafe fn try_read_output(&self, dst: *mut (), waker: &Waker) {
        let vtable = self.header().vtable;
        unsafe {
            (vtable.try_read_output)(self.0, dst, waker);
        }
    }
    pub(super) fn drop_join_handle_slow(&self) {
        let vtable = self.header().vtable;
        unsafe { (vtable.drop_join_handle_slow)(self.0) }
    }
    pub(super) fn shutdown(&self) {
        let vtable = self.header().vtable;
        unsafe { (vtable.shutdown)(self.0) }
    }
    pub(super) fn drop_reference(&self) {
        if self.state().ref_dec() {
            self.dealloc();
        }
    }
    /// This call consumes a ref-count and notifies the task. This will create a
    /// new Notified and submit it if necessary.
    ///
    /// The caller does not need to hold a ref-count besides the one that was
    /// passed to this call.
    pub(super) fn wake_by_val(&self) {
        match self.state().transition_to_notified_by_val() {
            TransitionToNotifiedByVal::Submit => {
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
    pub(super) fn wake_by_ref(&self) {
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
    pub(super) fn remote_abort(&self) {
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

unsafe impl Send for TaskRef {}
unsafe impl Sync for TaskRef {}

impl LocalTaskRef {
    /// Runs the task.
    pub(crate) fn poll(self) {
        let raw = self.task;
        raw.poll();
    }
}

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

        // Safety: we just allocated the stub so we know it's not a null ptr.
        log::trace!(
            "allocated task ptr {ptr:?} with layout {:?}",
            Layout::new::<Task<F, S>>()
        );
        Self {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
        }
    }

    unsafe fn poll(ptr: NonNull<Header>) {
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
        unsafe {
            let this = Self::from_raw(ptr);

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
                    let res = poll_future(this.core(), cx);

                    if res == Poll::Ready(()) {
                        // The future completed. Move on to complete the task.
                        return PollResult::Complete;
                    }

                    let transition_res = this.state().transition_to_idle();
                    if let TransitionToIdle::Cancelled = transition_res {
                        // The transition to idle failed because the task was
                        // cancelled during the poll.
                        cancel_task(this.core());
                    }
                    transition_result_to_poll_result(transition_res)
                }
                TransitionToRunning::Cancelled => {
                    cancel_task(this.core());
                    PollResult::Complete
                }
                TransitionToRunning::Failed => PollResult::Done,
                TransitionToRunning::Dealloc => PollResult::Dealloc,
            }
        }
    }

    unsafe fn schedule(ptr: NonNull<Header>) {
        unsafe {
            let this = Self::from_raw(ptr);
            this.core().scheduler.schedule(this.get_new_task());
        }
    }

    unsafe fn dealloc(ptr: NonNull<Header>) {
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
        unsafe {
            log::trace!(
                "about to dealloc task ptr {:?} with layout {:?}",
                ptr.as_ptr(),
                Layout::new::<Task<F, S>>()
            );
            drop(Box::from_raw(ptr.cast::<Task<F, S>>().as_ptr()));
            log::trace!("deallocated task")
        }
    }

    unsafe fn try_read_output(ptr: NonNull<Header>, dst: *mut (), waker: &Waker) {
        unsafe {
            let this = Self::from_raw(ptr);
            let dst = dst.cast::<Poll<super::Result<F::Output>>>();
            if can_read_output(this.header(), this.trailer(), waker) {
                *dst = Poll::Ready(this.core().take_output());
            }
        }
    }

    unsafe fn drop_join_handle_slow(ptr: NonNull<Header>) {
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

    unsafe fn from_raw(ptr: NonNull<Header>) -> Self {
        Self { ptr: ptr.cast() }
    }

    fn header_ptr(&self) -> NonNull<Header> {
        self.ptr.cast()
    }

    fn header(&self) -> &Header {
        unsafe { &*self.header_ptr().as_ptr() }
    }

    fn state(&self) -> &State {
        &self.header().state
    }

    fn core(&self) -> &Core<F, S> {
        unsafe { &self.ptr.as_ref().core }
    }

    fn trailer(&self) -> &Trailer {
        unsafe { &self.ptr.as_ref().trailer }
    }

    fn drop_reference(self) {
        if self.state().ref_dec() {
            unsafe {
                Self::dealloc(self.ptr.cast());
            }
        }
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
                unsafe {
                    self.core().drop_future_or_output();
                }
            } else if snapshot.is_join_waker_set() {
                // Notify the waker. Reading the waker field is safe per rule 4
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
    }

    /// Releases the task from the scheduler. Returns the number of ref-counts
    /// that should be decremented.
    fn release(&self) -> usize {
        // We don't actually increment the ref-count here, but the new task is
        // never destroyed, so that's ok.
        let me = ManuallyDrop::new(self.get_new_task());

        if let Some(task) = self.core().scheduler.release(&me) {
            mem::forget(task);
            2
        } else {
            1
        }
    }

    fn get_new_task(&self) -> TaskRef {
        // safety: The header is at the beginning of the cell, so this cast is
        // safe.
        unsafe { TaskRef::from_raw(self.ptr.cast()) }
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

        // Safety: we just allocated the stub so we know it's not a null ptr.
        Self {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
        }
    }

    unsafe fn poll_stub(_ptr: NonNull<Header>) {
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("poll_stub called on a stub task");
        }
    }

    unsafe fn schedule_stub(_ptr: NonNull<Header>) {
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("schedule_stub called on a stub task");
        }
    }

    unsafe fn try_read_output_stub(_ptr: NonNull<Header>, _dst: *mut (), _waker: &Waker) {
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("try_read_output_stub called on a stub task");
        }
    }

    unsafe fn drop_join_handle_slow_stub(_ptr: NonNull<Header>) {
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("drop_join_handle_slow_stub called on a stub task");
        }
    }

    unsafe fn shutdown_stub(_ptr: NonNull<Header>) {
        unsafe {
            debug_assert!(Header::get_id_ptr(_ptr).as_ref().is_stub());
            unreachable!("shutdown_stub called on a stub task");
        }
    }
}

impl Header {
    pub(super) unsafe fn get_id_ptr(me: NonNull<Header>) -> NonNull<Id> {
        unsafe {
            let offset = me.as_ref().vtable.id_offset;
            let id = me.as_ptr().cast::<u8>().add(offset).cast::<Id>();
            NonNull::new_unchecked(id)
        }
    }

    unsafe fn get_trailer_ptr(me: NonNull<Header>) -> NonNull<Trailer> {
        unsafe {
            let offset = me.as_ref().vtable.trailer_offset;
            let id = me.as_ptr().cast::<u8>().add(offset).cast::<Trailer>();
            NonNull::new_unchecked(id)
        }
    }
}

unsafe impl linked_list::Linked for Header {
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

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
        unsafe {
            Header::get_trailer_ptr(ptr)
                .map_addr(|addr| {
                    let offset = offset_of!(Trailer, owned_tasks_links);
                    addr.checked_add(offset).unwrap()
                })
                .cast()
        }
    }
}

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
        unsafe {
            Header::get_trailer_ptr(ptr)
                .map_addr(|addr| {
                    let offset = offset_of!(Trailer, run_queue_links);
                    addr.checked_add(offset).unwrap()
                })
                .cast()
        }
    }
}

impl<F: Future, S> Core<F, S> {
    /// Polls the future.
    ///
    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `state` field. This
    /// requires ensuring mutual exclusion between any concurrent thread that
    /// might modify the future or output field.
    ///
    /// The mutual exclusion is implemented by `Harness` and the `Lifecycle`
    /// component of the task state.
    ///
    /// `self` must also be pinned. This is handled by storing the task on the
    /// heap.
    pub(super) unsafe fn poll(&self, mut cx: Context<'_>) -> Poll<F::Output> {
        let res = {
            // Safety: The caller ensures mutual exclusion to the field.
            let future = match unsafe { &mut *self.stage.get() } {
                Stage::Running(future) => future,
                _ => unreachable!("unexpected stage"),
            };

            // Safety: The caller ensures the future is pinned.
            let future = unsafe { Pin::new_unchecked(future) };

            let _guard = TaskIdGuard::enter(self.task_id);
            future.poll(&mut cx)
        };

        if res.is_ready() {
            unsafe {
                self.drop_future_or_output();
            }
        }

        res
    }

    /// Drops the future.
    ///
    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `stage` field.
    pub(super) unsafe fn drop_future_or_output(&self) {
        // Safety: the caller ensures mutual exclusion to the field.
        unsafe {
            self.set_stage(Stage::Consumed);
        }
    }

    /// Stores the task output.
    ///
    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `stage` field.
    pub(super) unsafe fn store_output(&self, output: super::Result<F::Output>) {
        // Safety: the caller ensures mutual exclusion to the field.
        unsafe {
            self.set_stage(Stage::Finished(output));
        }
    }

    /// Takes the task output.
    ///
    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `stage` field.
    pub(super) unsafe fn take_output(&self) -> super::Result<F::Output> {
        // Safety:: the caller ensures mutual exclusion to the field.
        match mem::replace(unsafe { &mut *self.stage.get() }, Stage::Consumed) {
            Stage::Finished(output) => output,
            _ => panic!("JoinHandle polled after completion"),
        }
    }

    unsafe fn set_stage(&self, stage: Stage<F>) {
        let _guard = TaskIdGuard::enter(self.task_id);
        unsafe {
            *self.stage.get() = stage;
        }
    }
}

impl Trailer {
    pub(super) unsafe fn set_waker(&self, waker: Option<Waker>) {
        unsafe {
            *self.waker.get() = waker;
        }
    }

    pub(super) unsafe fn will_wake(&self, waker: &Waker) -> bool {
        unsafe { (*self.waker.get()).as_ref().unwrap().will_wake(waker) }
    }

    pub(super) unsafe fn wake_join(&self) {
        match unsafe { &*self.waker.get() } {
            Some(waker) => waker.wake_by_ref(),
            None => panic!("waker missing"),
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
            // same task, then return without touching the waker field. (Reading
            // the waker field below is safe per rule 3 in task/mod.rs.)
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
) -> core::result::Result<Snapshot, Snapshot> {
    assert!(snapshot.is_join_interested());
    assert!(!snapshot.is_join_waker_set());

    // Safety: Only the `JoinHandle` may set the `waker` field. When
    // `JOIN_INTEREST` is **not** set, nothing else will touch the field.
    unsafe {
        trailer.set_waker(Some(waker));
    }

    // Update the `JoinWaker` state accordingly
    let res = header.state.set_join_waker();

    // If the state could not be updated, then clear the join waker
    if res.is_err() {
        unsafe {
            trailer.set_waker(None);
        }
    }

    res
}

/// Cancels the task and store the appropriate error in the stage field.
fn cancel_task<T: Future, S>(core: &Core<T, S>) {
    // Drop the future from a panic guard.
    let res = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        core.drop_future_or_output();
    }));

    unsafe {
        core.store_output(Err(panic_result_to_join_error(core.task_id, res)));
    }
}

fn panic_result_to_join_error(
    task_id: Id,
    res: core::result::Result<(), Box<dyn Any + Send + 'static>>,
) -> JoinError {
    match res {
        Ok(()) => JoinError::cancelled(task_id),
        Err(panic) => JoinError::panic(task_id, panic),
    }
}

/// Polls the future. If the future completes, the output is written to the
/// stage field.
fn poll_future<T: Future, S: Schedule>(core: &Core<T, S>, cx: Context<'_>) -> Poll<()> {
    // Poll the future.
    let output = panic::catch_unwind(AssertUnwindSafe(|| {
        struct Guard<'a, T: Future, S: Schedule> {
            core: &'a Core<T, S>,
        }
        impl<'a, T: Future, S: Schedule> Drop for Guard<'a, T, S> {
            fn drop(&mut self) {
                // If the future panics on poll, we drop it inside the panic
                // guard.
                unsafe { self.core.drop_future_or_output(); }
            }
        }
        let guard = Guard { core };
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
    let res = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        core.store_output(output);
    }));

    if res.is_err() {
        panic!("unhandled panic {res:?}");
    }

    Poll::Ready(())
}

#[cold]
fn panic_to_error(task_id: Id, panic: Box<dyn Any + Send + 'static>) -> JoinError {
    log::error!("unhandled panic");
    // scheduler().unhandled_panic();
    JoinError::panic(task_id, panic)
}

/// Compute the offset of the `Core<F, S>` field in `Task<F, S>` using the
/// `#[repr(C)]` algorithm.
///
/// Pseudo-code for the `#[repr(C)]` algorithm can be found here:
/// <https://doc.rust-lang.org/reference/type-layout.html#reprc-structs>
const fn get_core_offset<F: Future, S>() -> usize {
    let mut offset = size_of::<Header>();

    let core_misalign = offset % align_of::<Core<F, S>>();
    if core_misalign > 0 {
        offset += align_of::<Core<F, S>>() - core_misalign;
    }

    offset
}

/// Compute the offset of the `Id` field in `Task<F, S>` using the
/// `#[repr(C)]` algorithm.
///
/// Pseudo-code for the `#[repr(C)]` algorithm can be found here:
/// <https://doc.rust-lang.org/reference/type-layout.html#reprc-structs>
const fn get_id_offset<F: Future, S>() -> usize {
    let mut offset = get_core_offset::<F, S>();
    offset += size_of::<S>();

    let id_misalign = offset % align_of::<Id>();
    if id_misalign > 0 {
        offset += align_of::<Id>() - id_misalign;
    }

    offset
}

/// Compute the offset of the `Trailer` field in `Cell<T, S>` using the
/// `#[repr(C)]` algorithm.
///
/// Pseudo-code for the `#[repr(C)]` algorithm can be found here:
/// <https://doc.rust-lang.org/reference/type-layout.html#reprc-structs>
const fn get_trailer_offset<F: Future, S>() -> usize {
    let mut offset = size_of::<Header>();

    let core_misalign = offset % align_of::<Core<F, S>>();
    if core_misalign > 0 {
        offset += align_of::<Core<F, S>>() - core_misalign;
    }
    offset += size_of::<Core<F, S>>();

    let trailer_misalign = offset % align_of::<Trailer>();
    if trailer_misalign > 0 {
        offset += align_of::<Trailer>() - trailer_misalign;
    }

    offset
}

/// Set and clear the task id in the context when the future is executed or
/// dropped, or when the output produced by the future is dropped.
pub(crate) struct TaskIdGuard {
    parent_task_id: Option<Id>,
}

impl TaskIdGuard {
    fn enter(_id: Id) -> Self {
        TaskIdGuard {
            // context::set_current_task_id(Some(id)),
            parent_task_id: None,
        }
    }
}

impl Drop for TaskIdGuard {
    fn drop(&mut self) {
        // context::set_current_task_id(self.parent_task_id);
    }
}
