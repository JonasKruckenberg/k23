// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::executor::task::id::Id;
use crate::executor::task::state::State;
use crate::executor::task::TaskRef;
use core::cell::UnsafeCell;
use core::future::Future;
use core::mem;
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};

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
/// [scheduler]: crate::executor::scheduler::Handle
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
pub(super) struct Task<F: Future, S> {
    pub(super) header: Header,
    pub(super) core: Core<F, S>,
    pub(super) trailer: Trailer,
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct Header {
    /// The task's state.
    ///
    /// This field is access with atomic instructions, so it's always safe to access it.
    pub(super) state: State,
    pub(super) vtable: &'static Vtable,
}

#[repr(C)]
#[derive(Debug)]
pub(super) struct Core<F: Future, S> {
    pub(super) scheduler: S,
    /// The future that the task is running.
    ///
    /// If `COMPLETE` is one, then the `JoinHandle` has exclusive access to this field
    /// If COMPLETE is zero, then the RUNNING bitfield functions as
    /// a lock for the stage field, and it can be accessed only by the thread
    /// that set RUNNING to one.
    pub(super) stage: UnsafeCell<Stage<F>>,
    pub(super) task_id: Id,
}

#[repr(C)]
#[derive(Debug)]
pub(super) struct Trailer {
    /// Consumer task waiting on completion of this task.
    ///
    /// This field may be access by different threads: on one hart we may complete a task and *read*
    /// the waker field to invoke the waker, and in another thread the task's `JoinHandle` may be
    /// polled, and if the task hasn't yet completed, the `JoinHandle` may *write* a waker to the
    /// waker field. The `JOIN_WAKER` bit in the headers`state` field ensures safe access by multiple
    /// hart to the waker field using the following rules:
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
    pub(super) waker: UnsafeCell<Option<Waker>>,
    /// Links to other tasks in the intrusive global run queue.
    ///
    /// TODO ownership
    pub(super) run_queue_links: mpsc_queue::Links<Header>,
    /// Links to other tasks in the global "owned tasks" list.
    ///
    /// The `OwnedTask` reference has exclusive access to this field.
    pub(super) owned_tasks_links: linked_list::Links<Header>,
}

#[derive(Debug)]
pub(super) struct Vtable {
    /// Polls the future.
    pub(super) poll: unsafe fn(NonNull<Header>),
    /// Schedules the task for execution on the runtime.
    pub(super) schedule: unsafe fn(NonNull<Header>),
    /// Deallocates the memory.
    pub(super) dealloc: unsafe fn(NonNull<Header>),
    /// Reads the task output, if complete.
    pub(super) try_read_output: unsafe fn(NonNull<Header>, *mut (), &Waker),
    /// The join handle has been dropped.
    pub(super) drop_join_handle_slow: unsafe fn(NonNull<Header>),
    /// Scheduler is being shutdown.
    pub(super) shutdown: unsafe fn(NonNull<Header>),
    /// The number of bytes that the `id` field is offset from the header.
    pub(super) id_offset: usize,
    /// The number of bytes that the `trailer` field is offset from the header.
    pub(super) trailer_offset: usize,
}

/// Either the future or the output.
#[repr(C)] // https://github.com/rust-lang/miri/issues/3780
pub(super) enum Stage<T: Future> {
    Running(T),
    Finished(super::Result<T::Output>),
    Consumed,
}

impl Header {
    /// # Safety
    ///
    /// The caller must ensure the pointer is valid
    pub(super) unsafe fn get_id_ptr(me: NonNull<Header>) -> NonNull<Id> {
        // Safety: validity of `me` ensured by caller and the rest is ensured by construction through the vtable
        unsafe {
            let offset = me.as_ref().vtable.id_offset;
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "`get_id_offset` ensures the offset is aligned correctly"
            )]
            let id = me.as_ptr().cast::<u8>().add(offset).cast::<Id>();
            NonNull::new_unchecked(id)
        }
    }

    /// # Safety
    ///
    /// The caller must ensure the pointer is valid
    unsafe fn get_trailer_ptr(me: NonNull<Header>) -> NonNull<Trailer> {
        // Safety: validity of `me` ensured by caller and the rest is ensured by construction through the vtable
        unsafe {
            let offset = me.as_ref().vtable.trailer_offset;
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "`get_trailer_offset` ensures the offset is aligned correctly"
            )]
            let id = me.as_ptr().cast::<u8>().add(offset).cast::<Trailer>();
            NonNull::new_unchecked(id)
        }
    }
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
        // Safety: ensured by the caller
        unsafe { TaskRef::from_raw(ptr) }
    }
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
        // Safety: `TaskRef` is just a newtype wrapper around `NonNull<Header>`
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

// Safety: tasks are always treated as pinned in memory (a requirement for polling them)
// and care has been taken below to ensure the underlying memory isn't freed as long as the
// `TaskRef` is part of the queue.
unsafe impl mpsc_queue::Linked for Header {
    type Handle = TaskRef;

    fn into_ptr(task: Self::Handle) -> NonNull<Self> {
        let ptr = task.header_ptr();
        // converting a `TaskRef` into a pointer to enqueue it assigns ownership
        // of the ref count to the queue, so we don't want to run its `Drop`
        // impl.
        mem::forget(task);
        ptr
    }
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: ensured by the caller
        unsafe { TaskRef::from_raw(ptr) }
    }
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<mpsc_queue::Links<Self>>
    where
        Self: Sized,
    {
        // Safety: `TaskRef` is just a newtype wrapper around `NonNull<Header>`
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
    /// `self` must also be pinned. This is handled by storing the task on the
    /// heap.
    pub(super) unsafe fn poll(&self, mut cx: Context<'_>) -> Poll<F::Output> {
        let res = {
            // Safety: The caller ensures mutual exclusion
            let stage = unsafe { &mut *self.stage.get() };
            let Stage::Running(future) = stage else {
                unreachable!("unexpected stage");
            };

            // Safety: The caller ensures the future is pinned.
            let future = unsafe { Pin::new_unchecked(future) };
            future.poll(&mut cx)
        };

        if res.is_ready() {
            // Safety: The caller ensures mutual exclusion
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
    /// The caller must ensure it is safe to mutate the `stage` field. This requires ensuring mutual
    /// exclusion between any concurrent thread that might modify the future or output field.
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
    /// The caller must ensure it is safe to mutate the `stage` field. This requires ensuring mutual
    /// exclusion between any concurrent thread that might modify the future or output field.
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
    /// The caller must ensure it is safe to mutate the `stage` field. This requires ensuring mutual
    /// exclusion between any concurrent thread that might modify the future or output field.
    pub(super) unsafe fn take_output(&self) -> super::Result<F::Output> {
        // Safety:: the caller ensures mutual exclusion to the field.
        match mem::replace(unsafe { &mut *self.stage.get() }, Stage::Consumed) {
            Stage::Finished(output) => output,
            _ => panic!("JoinHandle polled after completion"),
        }
    }

    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `stage` field. This requires ensuring mutual
    /// exclusion between any concurrent thread that might modify the future or output field.
    unsafe fn set_stage(&self, stage: Stage<F>) {
        // Safety: ensured by the caller
        unsafe {
            *self.stage.get() = stage;
        }
    }
}

impl Trailer {
    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `waker` field. This requires ensuring mutual
    /// exclusion between any concurrent thread that might modify the field.
    pub(super) unsafe fn set_waker(&self, waker: Option<Waker>) {
        // Safety: ensured by the caller
        unsafe {
            *self.waker.get() = waker;
        }
    }

    /// # Safety
    ///
    /// The caller must ensure it is safe to mutate the `waker` field. This requires ensuring mutual
    /// exclusion between any concurrent thread that might modify the field.
    pub(super) unsafe fn will_wake(&self, waker: &Waker) -> bool {
        // Safety: ensured by the caller
        unsafe { (*self.waker.get()).as_ref().unwrap().will_wake(waker) }
    }

    /// # Safety
    ///
    /// The caller must ensure it is safe to read the `waker` field.
    pub(super) unsafe fn wake_join(&self) {
        // Safety: ensured by the caller
        match unsafe { &*self.waker.get() } {
            Some(waker) => waker.wake_by_ref(),
            None => panic!("waker missing"),
        }
    }
}

/// Compute the offset of the `Core<F, S>` field in `Task<F, S>` using the
/// `#[repr(C)]` algorithm.
///
/// Pseudo-code for the `#[repr(C)]` algorithm can be found here:
/// <https://doc.rust-lang.org/reference/type-layout.html#reprc-structs>
pub const fn get_core_offset<F: Future, S>() -> usize {
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
pub const fn get_id_offset<F: Future, S>() -> usize {
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
pub const fn get_trailer_offset<F: Future, S>() -> usize {
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
