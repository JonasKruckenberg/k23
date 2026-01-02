// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem::ManuallyDrop;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use futures::pin_mut;
use futures::task::WakerRef;
use k32_util::loom_const_fn;

use crate::loom::sync::{Arc, Condvar, Mutex as StdMutex};

#[derive(Debug)]
pub struct ThreadNotify {
    mutex: StdMutex<bool>,
    condvar: Condvar,
}

impl ThreadNotify {
    loom_const_fn! {
        const fn new() -> Self {
            Self {
                mutex: StdMutex::new(false),
                condvar: Condvar::new()
            }
        }
    }

    #[tracing::instrument(level = "trace", skip(self))]
    fn wait(&self) {
        let mut notified = self.mutex.lock().unwrap();
        while !*notified {
            notified = self.condvar.wait(notified).unwrap();
        }
        *notified = false;
    }

    fn notify(&self) {
        let mut signaled = self.mutex.lock().unwrap();
        *signaled = true;
        self.condvar.notify_one();
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
        ThreadNotify::notify(&arc);
    }

    // used by `waker_ref`
    unsafe fn wake_by_ref_arc_raw(data: *const ()) {
        // Retain Arc, but don't touch refcount by wrapping in ManuallyDrop
        let arc = ManuallyDrop::new(unsafe { Arc::from_raw(data.cast::<ThreadNotify>()) });
        ThreadNotify::notify(&arc);
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
        static CURRENT_THREAD_NOTIFY: Arc<ThreadNotify> = Arc::new(ThreadNotify::new());
    }

    CURRENT_THREAD_NOTIFY.with(|thread_notify| {
        let waker = waker_ref(&thread_notify);
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(t) = f.as_mut().poll(&mut cx) {
                return t;
            }

            thread_notify.wait();
        }
    })
}
