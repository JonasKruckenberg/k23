// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::sync::Arc;
use core::arch::asm;
use core::mem::ManuallyDrop;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use futures::pin_mut;
use futures::task::WakerRef;
use kcpu_local::cpu_local;
use riscv::sbi;

use crate::state;

struct HartNotify {
    hartid: usize,
    unparked: AtomicBool,
}

impl HartNotify {
    fn wake_by_ref(me: &Arc<Self>) {
        tracing::trace!("waking up hart {}...", me.hartid);

        // Make sure the wakeup is remembered until the next `park()`.
        let unparked = me.unparked.swap(true, Ordering::Release);
        if !unparked {
            // If the thread has not been unparked yet, it must be done
            // now. If it was actually parked, it will run again,
            // otherwise the token made available by `unpark`
            // may be consumed before reaching `park()`, but `unparked`
            // ensures it is not forgotten.
            sbi::ipi::send_ipi(1 << me.hartid, 0).unwrap();
        }
    }
}

fn waker_ref(wake: &Arc<HartNotify>) -> WakerRef<'_> {
    static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        clone_arc_raw,
        wake_arc_raw,
        wake_by_ref_arc_raw,
        drop_arc_raw,
    );

    unsafe fn clone_arc_raw(data: *const ()) -> RawWaker {
        // Safety: ensured by caller
        unsafe { Arc::increment_strong_count(data.cast::<HartNotify>()) }
        RawWaker::new(data, &WAKER_VTABLE)
    }

    unsafe fn wake_arc_raw(data: *const ()) {
        // Safety: the `data` pointer is created through `Arc::as_ptr` below.
        let arc = unsafe { Arc::from_raw(data.cast::<HartNotify>()) };
        HartNotify::wake_by_ref(&arc);
    }

    unsafe fn wake_by_ref_arc_raw(data: *const ()) {
        // Retain Arc, but don't touch refcount by wrapping in ManuallyDrop
        // Safety: the `data` pointer is created through `Arc::as_ptr` below.
        let arc = ManuallyDrop::new(unsafe { Arc::from_raw(data.cast::<HartNotify>()) });
        HartNotify::wake_by_ref(&arc);
    }

    unsafe fn drop_arc_raw(data: *const ()) {
        // Safety: the `data` pointer is created through `Arc::as_ptr` below.
        drop(unsafe { Arc::from_raw(data.cast::<HartNotify>()) });
    }

    // simply copy the pointer instead of using Arc::into_raw,
    // as we don't actually keep a refcount by using ManuallyDrop.<
    let ptr = Arc::as_ptr(wake).cast::<()>();

    // Safety: TODO
    let waker = ManuallyDrop::new(unsafe { Waker::from_raw(RawWaker::new(ptr, &WAKER_VTABLE)) });
    WakerRef::new_unowned(waker)
}

/// Blocks the calling hart until the give future completes and yielding its
/// resolved result.
///
/// Any tasks or timers which the future spawns internally, will **not** be executed unless there
/// are running executor workers available. This makes this function mostly suitable to drive
/// the top-level main loop future returned by `Worker::run`, since that will take care of
/// driving the tasks and timers to completion.
///
/// When the given future cannot make progress and returned `Poll::Pending`, the calling hart
/// will automatically be put into a low-power suspend state to conserve energy by using the builtin
/// "wait for interrupt" (`wfi`) instruction until woken by the [`Waker`]. When using this function
/// to drive the worker loop the `Waker` should be called for event that might result in tasks becoming
/// unblocked such as interrupts and "wakeup calls" from other harts.
pub fn block_on<F: Future>(f: F) -> F::Output {
    pin_mut!(f);

    cpu_local! {
        static CURRENT_THREAD_NOTIFY: Arc<HartNotify> = Arc::new(HartNotify {
            hartid: state::cpu_local().id,
            unparked: AtomicBool::new(false),
        });
    }

    let waker = waker_ref(&CURRENT_THREAD_NOTIFY);
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(t) = f.as_mut().poll(&mut cx) {
            return t;
        }

        // Wait for a wakeup.
        while !CURRENT_THREAD_NOTIFY
            .unparked
            .swap(false, Ordering::Acquire)
        {
            // No wakeup occurred. It may occur now, right before parking,
            // but in that case the token made available by `unpark()`
            // is guaranteed to still be available and `park()` is a no-op.
            tracing::trace!("parking hart {}", state::cpu_local().id);
            // Safety: inline assembly
            unsafe { asm!("wfi") };
            tracing::trace!("hart {} woke up", state::cpu_local().id);
        }
    }
}
