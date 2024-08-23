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
    sstatus::set_sie()
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
    enable();

    let r = f();

    // If the interrupts were inactive before our `enable` call, then re-disable
    // them. Otherwise, keep them enabled
    if !sstatus.sie() {
        disable();
    }

    // Restore SSTATUS.SPIE, SSTATUS.SPP, and SEPC
    if sstatus.spie() {
        sstatus::set_spie();
    }
    sstatus::set_spp(sstatus.spp());
    sepc::write(sepc.as_bits());

    r
}
