// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::executor::Executor;
use crate::loom;
use crate::loom::sync::atomic::{AtomicBool, Ordering};
use crate::loom::sync::{Arc, Condvar};
use crate::loom::thread;
use crate::loom::thread::{JoinHandle, Thread};
use crate::time::{Clock, Instant, PhysTicks, Timer};
use core::mem::ManuallyDrop;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use futures::pin_mut;
use futures::task::WakerRef;
use util::loom_const_fn;

#[derive(Debug)]
pub struct ThreadNotify {
    thread: Thread,
    unparked: AtomicBool,
}

impl ThreadNotify {
    loom_const_fn! {
        const fn new(thread: Thread) -> Self {
            Self {
                thread,
                unparked: AtomicBool::new(false),
            }
        }
    }

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
        static CURRENT_THREAD_NOTIFY: Arc<ThreadNotify> = Arc::new(ThreadNotify::new(thread::current()));
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

/// Constructs a new [`Clock`] that produces arbitrary, monotonically nondecreasing timestamps
/// with a 100ns precision.
pub fn clock_100ns() -> Clock {
    loom::lazy_static! {
        static ref TIME_ANCHOR: std::time::Instant = std::time::Instant::now();
    }

    Clock::new(std::time::Duration::from_nanos(100), move || {
        let elapsed = TIME_ANCHOR.elapsed();
        PhysTicks(elapsed.as_nanos() as u64 / 100)
    })
}

/// Constructs a new [`Clock`] that produces arbitrary, monotonically nondecreasing timestamps
/// with a 1ms precision.
pub fn clock_1ms() -> Clock {
    loom::lazy_static! {
        static ref TIME_ANCHOR: std::time::Instant = std::time::Instant::now();
    }

    Clock::new(std::time::Duration::from_millis(1), move || {
        let elapsed = TIME_ANCHOR.elapsed();
        PhysTicks(elapsed.as_millis() as u64)
    })
}

#[derive(Debug, Clone)]
pub struct TimeDriverHandle(Arc<(loom::sync::Mutex<bool>, Condvar)>);

impl TimeDriverHandle {
    pub fn wake(&self) {
        let (running, cvar) = &*self.0;
        debug_assert!(*running.lock().unwrap());
        cvar.notify_one();
    }

    pub fn close(self) {
        let (running, cvar) = &*self.0;
        let mut running = running.lock().unwrap();
        *running = false;
        cvar.notify_all();
    }
}

pub fn spawn_time_driver(
    timer: &'static Timer,
    exec: &'static Executor,
) -> (TimeDriverHandle, JoinHandle<()>) {
    let pair = Arc::new((loom::sync::Mutex::new(true), Condvar::new()));

    let pair2 = pair.clone();
    let h = thread::spawn(move || {
        let (running, cvar) = &*pair2;
        let mut running = running.lock().unwrap();

        while *running {
            tracing::trace!("turning timer...");

            let (expired, maybe_next_deadline) = timer.turn();

            let now = Instant::now(timer);

            if expired > 0 {
                exec.wake_one();
            }

            if let Some(next_deadline) = maybe_next_deadline {
                let instant = next_deadline.as_instant(timer);
                let timeout = instant.duration_since(now);

                tracing::trace!("parking time loop for {timeout:?}...");
                running = cvar.wait_timeout(running, timeout).unwrap().0;
            } else {
                tracing::trace!("parking time loop...");
                running = cvar.wait(running).unwrap();
            }
        }
    });

    (TimeDriverHandle(pair), h)
}
