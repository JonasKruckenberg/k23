// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::{AtomicPtr, Ordering};
use crate::time::{TimeError, Timer};
use core::ptr;

static GLOBAL_TIMER: AtomicPtr<Timer> = AtomicPtr::new(ptr::null_mut());

/// Errors returned by [`set_global_timer`].
#[derive(Debug)]
pub struct AlreadyInitialized(());

/// Sets a [`Timer`] as the [global default timer].
///
/// This function must be called in order for the [`sleep`] and [`timeout`] free
/// functions to be used.
///
/// The global timer can only be set a single time. Once the global timer is
/// initialized, subsequent calls to this function will return an
/// [`AlreadyInitialized`] error.
///
/// [`sleep`]: crate::time::sleep()
/// [`timeout`]: crate::time::timeout()
/// [global default timer]: crate::time#global-timers
pub fn set_global_timer(timer: &'static Timer) -> Result<(), AlreadyInitialized> {
    GLOBAL_TIMER
        .compare_exchange(
            ptr::null_mut(),
            ptr::from_ref(timer).cast_mut(),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map_err(|_| AlreadyInitialized(()))
        .map(|_| ())
}

#[inline(always)]
pub(in crate::time) fn global_timer() -> Result<&'static Timer, TimeError> {
    let ptr = GLOBAL_TIMER.load(Ordering::Acquire);
    ptr::NonNull::new(ptr)
        .ok_or(TimeError::NoGlobalTimer)
        .map(|ptr| unsafe {
            // safety: we have just null-checked this pointer, so we know it's not
            // null. and it's safe to convert it to an `&'static Timer`, because we
            // know that the pointer stored in the atomic *came* from an `&'static
            // Timer` (as it's only set in `set_global_timer`).
            ptr.as_ref()
        })
}
