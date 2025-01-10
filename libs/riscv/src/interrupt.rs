// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Interrupts

use crate::{sepc, sstatus};

/// Disables all interrupts for the current hart.
#[inline]
pub fn disable() {
    // SAFETY: It is safe to disable interrupts
    unsafe { sstatus::clear_sie() }
}

/// Enables all the interrupts for the current hart.
///
/// # Safety
///
/// The caller must ensure the remaining code is signal-safe.
#[inline]
pub unsafe fn enable() {
    unsafe { sstatus::set_sie() }
}

/// Execute closure `f` with interrupts disabled for the current hart.
#[inline]
pub fn with_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let sstatus = sstatus::read();

    // disable interrupts
    disable();

    let r = f();

    // If the interrupts were active before our `disable` call, then re-enable
    // them. Otherwise, keep them disabled
    if sstatus.sie() {
        unsafe { enable() };
    }

    r
}

/// Execute closure `f` with interrupts enabled for the current hart.
///
/// This function is designed to be run from within an interrupt handler to allow for recursive interrupts.
///
/// # Safety
///
/// - The caller must ensure the remaining code is signal-safe.
/// - The interrupt flag must be cleared before calling this function, otherwise the interrupt handler will be re-entered.
#[inline]
pub unsafe fn with_enabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let sstatus = sstatus::read();
    let sepc = sepc::read();

    // enable interrupts to allow nested interrupts
    unsafe { enable() };

    let r = f();

    // If the interrupts were inactive before our `enable` call, then re-disable
    // them. Otherwise, keep them enabled
    if !sstatus.sie() {
        disable();
    }

    // Restore SSTATUS.SPIE, SSTATUS.SPP, and SEPC
    if sstatus.spie() {
        unsafe { sstatus::set_spie() };
    }
    unsafe { sstatus::set_spp(sstatus.spp()) };
    sepc::set(sepc);

    r
}
