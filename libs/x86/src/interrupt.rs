// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

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