// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ops;
use core::ptr::NonNull;
use core::task::{RawWaker, RawWakerVTable, Waker};
use super::raw::{Header};
use crate::scheduler2::task::raw::TaskRef;

pub(super) struct WakerRef<'a> {
    waker: ManuallyDrop<Waker>,
    _p: PhantomData<&'a Header>,
}

/// Returns a `WakerRef` which avoids having to preemptively increase the
/// refcount if there is no need to do so.
pub(super) fn waker_ref(header: &NonNull<Header>) -> WakerRef<'_> {
    // `Waker::will_wake` uses the VTABLE pointer as part of the check. This
    // means that `will_wake` will always return false when using the current
    // task's waker. (discussion at rust-lang/rust#66281).
    //
    // To fix this, we use a single vtable. Since we pass in a reference at this
    // point and not an *owned* waker, we must ensure that `drop` is never
    // called on this waker instance. This is done by wrapping it with
    // `ManuallyDrop` and then never calling drop.
    let waker = unsafe { ManuallyDrop::new(Waker::from_raw(raw_waker(*header))) };

    WakerRef {
        waker,
        _p: PhantomData,
    }
}

impl ops::Deref for WakerRef<'_> {
    type Target = Waker;

    fn deref(&self) -> &Waker {
        &self.waker
    }
}

unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
    unsafe {
        let header = NonNull::new_unchecked(ptr as *mut Header);
        header.as_ref().state.ref_inc();
        raw_waker(header)
    }
}

unsafe fn drop_waker(ptr: *const ()) {
    unsafe {
        let ptr = NonNull::new_unchecked(ptr as *mut Header);
        let raw = TaskRef::from_raw(ptr);
        raw.drop_reference();
    }
}

unsafe fn wake_by_val(ptr: *const ()) {
    unsafe {
        let ptr = NonNull::new_unchecked(ptr as *mut Header);
        let raw = TaskRef::from_raw(ptr);
        raw.wake_by_val();
    }
}

// Wake without consuming the waker
unsafe fn wake_by_ref(ptr: *const ()) {
    unsafe {
        let ptr = NonNull::new_unchecked(ptr as *mut Header);
        let raw = TaskRef::from_raw(ptr);
        raw.wake_by_ref();
    }
}

static WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_waker, wake_by_val, wake_by_ref, drop_waker);

fn raw_waker(header: NonNull<Header>) -> RawWaker {
    let ptr = header.as_ptr() as *const ();
    RawWaker::new(ptr, &WAKER_VTABLE)
}