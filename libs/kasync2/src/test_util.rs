// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::{AtomicBool, Ordering};
use crate::loom::sync::Arc;
use crate::loom::thread;
use crate::loom::thread::Thread;
use core::mem::ManuallyDrop;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use futures::pin_mut;
use futures::task::WakerRef;

struct ThreadNotify {
    thread: Thread,
    unparked: AtomicBool,
}

impl ThreadNotify {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        // Make sure the wakeup is remembered until the next `park()`.
        let unparked = arc_self.unparked.swap(true, Ordering::Release);
        if !unparked {
            // If the thread has not been unparked yet, it must be done
            // now. If it was actually parked, it will run again,
            // otherwise the token made available by `unpark`
            // may be consumed before reaching `park()`, but `unparked`
            // ensures it is not forgotten.
            arc_self.thread.unpark();
        }
    }
}

fn waker_ref(wake: &Arc<ThreadNotify>) -> WakerRef<'_> {
    static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        clone_arc_raw,
        wake_arc_raw,
        wake_by_ref_arc_raw,
        drop_arc_raw,
    );

    unsafe fn clone_arc_raw(data: *const ()) -> RawWaker {
        unsafe { Arc::increment_strong_count(data.cast::<ThreadNotify>()) }
        RawWaker::new(data, &WAKER_VTABLE)
    }

    unsafe fn wake_arc_raw(data: *const ()) {
        let arc = unsafe { Arc::from_raw(data.cast::<ThreadNotify>()) };
        ThreadNotify::wake_by_ref(&arc);
    }

    // used by `waker_ref`
    unsafe fn wake_by_ref_arc_raw(data: *const ()) {
        // Retain Arc, but don't touch refcount by wrapping in ManuallyDrop
        let arc = ManuallyDrop::new(unsafe { Arc::from_raw(data.cast::<ThreadNotify>()) });
        ThreadNotify::wake_by_ref(&arc);
    }

    unsafe fn drop_arc_raw(data: *const ()) {
        drop(unsafe { Arc::from_raw(data.cast::<ThreadNotify>()) })
    }

    // simply copy the pointer instead of using Arc::into_raw,
    // as we don't actually keep a refcount by using ManuallyDrop.<
    let ptr = Arc::as_ptr(wake).cast::<()>();

    let waker = ManuallyDrop::new(unsafe { Waker::from_raw(RawWaker::new(ptr, &WAKER_VTABLE)) });
    WakerRef::new_unowned(waker)
}

pub fn block_on<F: Future>(f: F) -> F::Output {
    pin_mut!(f);

    crate::loom::thread_local! {
        static CURRENT_THREAD_NOTIFY: Arc<ThreadNotify> = Arc::new(ThreadNotify {
            thread: thread::current(),
            unparked: AtomicBool::new(false),
        });
    }

    CURRENT_THREAD_NOTIFY.with(|thread_notify| {
        let waker = waker_ref(&thread_notify);
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(t) = f.as_mut().poll(&mut cx) {
                return t;
            }

            // Wait for a wakeup.
            while !thread_notify.unparked.swap(false, Ordering::Acquire) {
                // No wakeup occurred. It may occur now, right before parking,
                // but in that case the token made available by `unpark()`
                // is guaranteed to still be available and `park()` is a no-op.
                thread::park();
            }
        }
    })
}

/// Returns a [`Clock`] with 1ms precision that is backed by the system clock
#[macro_export]
macro_rules! std_clock {
    () => {{
        crate::loom::lazy_static! {
            static ref TIME_ANCHOR: ::std::time::Instant = ::std::time::Instant::now();
        }

        $crate::time::Clock::new(::core::time::Duration::from_millis(1), move || {
            $crate::time::Ticks(TIME_ANCHOR.elapsed().as_millis() as u64)
        })
    }};
}
pub use std_clock;
