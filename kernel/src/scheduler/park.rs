// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{RawWaker, RawWakerVTable, Waker};

const EMPTY: usize = 0;
const PARKED: usize = 1;
const NOTIFIED: usize = 2;

struct Inner {
    state: AtomicUsize,
    cpuid: usize,
}

impl Inner {
    fn park(&self) {
        assert!(crate::BOOT_INFO.get().unwrap().cpu_mask.count_ones() > 1);
        
        // If we were previously notified then we consume this notification and
        // return quickly.
        if self
            .state
            .compare_exchange(NOTIFIED, EMPTY, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return;
        }

        match self
            .state
            .compare_exchange(EMPTY, PARKED, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => {}
            Err(NOTIFIED) => {
                // We must read here, even though we know it will be `NOTIFIED`.
                // This is because `unpark` may have been called again since we read
                // `NOTIFIED` in the `compare_exchange` above. We must perform an
                // acquire operation that synchronizes with that `unpark` to observe
                // any writes it made before the call to unpark. To do that we must
                // read from the write it made to `state`.
                let old = self.state.swap(EMPTY, Ordering::SeqCst);
                debug_assert_eq!(old, NOTIFIED, "park state changed unexpectedly");

                return;
            }
            Err(actual) => panic!("inconsistent park state; actual = {actual}"),
        }

        loop {
            // Safety: we have to trust that someone will wake us up eventually..
            unsafe { arch::cpu_park() };

            if self
                .state
                .compare_exchange(NOTIFIED, EMPTY, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                // got a notification
                return;
            }

            // spurious wakeup, go back to sleep
        }
    }

    fn unpark(&self) {
        match self.state.swap(NOTIFIED, Ordering::SeqCst) {
            EMPTY => return,    // no one was waiting
            NOTIFIED => return, // already unparked
            PARKED => {}        // gotta go wake up the CPU
            _ => panic!("inconsistent state in unpark"),
        }

        // Safety: we checked above that the CPU is indeed parked
        unsafe { arch::cpu_unpark(self.cpuid) }
    }

    fn shutdown(&self) {
        unsafe { arch::cpu_unpark(self.cpuid) }
    }

    #[allow(clippy::wrong_self_convention)]
    fn into_raw(this: Arc<Self>) -> *const () {
        Arc::into_raw(this) as *const ()
    }

    #[allow(clippy::wrong_self_convention)]
    fn into_raw_waker(this: Arc<Self>) -> RawWaker {
        RawWaker::new(
            Inner::into_raw(this),
            &RawWakerVTable::new(clone, wake, wake_by_ref, drop_waker),
        )
    }

    unsafe fn from_raw(ptr: *const ()) -> Arc<Self> {
        // Safety: ensured by caller
        unsafe { Arc::from_raw(ptr as *const Self) }
    }
}

#[derive(Clone)]
pub struct ParkToken(Arc<Inner>);
impl ParkToken {
    pub fn new(cpuid: usize) -> Self {
        Self(Arc::new(Inner {
            state: AtomicUsize::new(EMPTY),
            cpuid,
        }))
    }

    pub fn cpuid(&self) -> usize {
        self.0.cpuid
    }

    pub fn park(&self) {
        self.0.park();
    }

    pub fn into_unpark(self) -> UnparkToken {
        UnparkToken(self.0)
    }
}

pub struct UnparkToken(Arc<Inner>);
impl UnparkToken {
    pub fn cpuid(&self) -> usize {
        self.0.cpuid
    }

    pub fn unpark(self) {
        self.0.unpark();
    }

    pub fn into_waker(self) -> Waker {
        unsafe {
            let raw = Inner::into_raw_waker(self.0);
            Waker::from_raw(raw)
        }
    }
}

unsafe fn clone(raw: *const ()) -> RawWaker {
    // Safety: ensured by VTable
    unsafe {
        Arc::increment_strong_count(raw as *const Inner);
        Inner::into_raw_waker(Inner::from_raw(raw))
    }
}

unsafe fn drop_waker(raw: *const ()) {
    // Safety: ensured by VTable
    unsafe {
        drop(Inner::from_raw(raw));
    }
}

unsafe fn wake(raw: *const ()) {
    // Safety: ensured by VTable
    let unparker = unsafe { Inner::from_raw(raw) };
    unparker.unpark();
}

unsafe fn wake_by_ref(raw: *const ()) {
    let raw = raw as *const Inner;
    // Safety: ensured by VTable
    unsafe {
        (*raw).unpark();
    }
}
