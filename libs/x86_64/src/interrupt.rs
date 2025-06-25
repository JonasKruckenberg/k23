// new file

//! x86_64 interrupt control

use core::arch::asm;

/// Disables all interrupts for the current CPU core.
#[inline]
pub fn disable() {
    // SAFETY: It is safe to disable interrupts
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
}

/// Enables all the interrupts for the current CPU core.
///
/// # Safety
///
/// The caller must ensure the remaining code is signal-safe.
#[inline]
pub unsafe fn enable() {
    unsafe {
        asm!("sti", options(nomem, nostack));
    }
}