// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::raw::Header;
use crate::executor::task::TaskRef;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ptr::NonNull;
use core::task::{RawWaker, RawWakerVTable, Waker};
use core::{mem, ops};

pub(super) struct WakerRef<'a, S: 'static> {
    waker: ManuallyDrop<Waker>,
    _p: PhantomData<(&'a Header, S)>,
}

/// Returns a `WakerRef` which avoids having to preemptively increase the
/// refcount if there is no need to do so.
pub(super) fn waker_ref<S>(header: &NonNull<Header>) -> WakerRef<'_, S> {
    // `Waker::will_wake` uses the VTABLE pointer as part of the check. This
    // means that `will_wake` will always return false when using the current
    // task's waker. (discussion at rust-lang/rust#66281).
    //
    // To fix this, we use a single vtable. Since we pass in a reference at this
    // point and not an *owned* waker, we must ensure that `drop` is never
    // called on this waker instance. This is done by wrapping it with
    // `ManuallyDrop` and then never calling drop.
    // Safety: the raw waker and its vtable are constructed below
    let waker = unsafe { ManuallyDrop::new(Waker::from_raw(raw_waker(*header))) };

    WakerRef {
        waker,
        _p: PhantomData,
    }
}

impl<S> ops::Deref for WakerRef<'_, S> {
    type Target = Waker;

    fn deref(&self) -> &Waker {
        &self.waker
    }
}

/// # Safety
///
/// The caller has to ensure `ptr` is a valid pointer to a task
unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
    // Safety: caller has to ensure `ptr` is a valid pointer to a task
    unsafe {
        let header = NonNull::new_unchecked(ptr as *mut Header);
        log::trace!("waker.clone_waker {ptr:?}");
        header.as_ref().state.ref_inc();
        raw_waker(header)
    }
}

/// # Safety
///
/// The caller has to ensure `ptr` is a valid pointer to a task
unsafe fn drop_waker(ptr: *const ()) {
    // Safety: caller has to ensure `ptr` is a valid pointer to a task
    unsafe {
        let ptr = NonNull::new_unchecked(ptr as *mut Header);
        log::trace!("waker.drop_waker {ptr:?}");
        let raw = TaskRef::from_raw(ptr);
        raw.drop_reference();
    }
}

/// Wake with consuming the waker
///
/// # Safety
///
/// The caller has to ensure `ptr` is a valid pointer to a task
unsafe fn wake_by_val(ptr: *const ()) {
    // Safety: caller has to ensure `ptr` is a valid pointer to a task
    unsafe {
        let ptr = NonNull::new_unchecked(ptr as *mut Header);
        log::trace!("waker.wake_by_val {ptr:?}");
        let raw = TaskRef::from_raw(ptr);
        raw.wake_by_val();
    }
}

/// Wake without consuming the waker
///
/// # Safety
///
/// The caller has to ensure `ptr` is a valid pointer to a task
unsafe fn wake_by_ref(ptr: *const ()) {
    // Safety: caller has to ensure `ptr` is a valid pointer to a task
    unsafe {
        let ptr = NonNull::new_unchecked(ptr as *mut Header);
        log::trace!("waker.wake_by_ref {ptr:?}");
        let task = TaskRef::from_raw(ptr);
        task.wake_by_ref();
        // Prevent dropping the `task` reference because we just fabricated that out of thin air
        // we could also call `TaskRef::clone_from_raw` above and allow the drop here, but that
        // would basically do two atomic operations for no real reason
        mem::forget(task);
    }
}

static WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_waker, wake_by_val, wake_by_ref, drop_waker);

fn raw_waker(header: NonNull<Header>) -> RawWaker {
    let ptr = header.as_ptr() as *const ();
    RawWaker::new(ptr, &WAKER_VTABLE)
}
