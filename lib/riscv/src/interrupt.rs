// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Interrupts
#![expect(clippy::undocumented_unsafe_blocks, reason = "register access")]

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
///
/// The previous interrupt state is restored once `f` returns, including when it
/// unwinds — hence the guard rather than a plain restore after the call. A
/// panic escaping `f` would otherwise leave the hart with interrupts masked for
/// good.
#[inline]
pub fn with_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    /// Re-enables interrupts if they were enabled when it was created.
    struct Restore(bool);

    impl Drop for Restore {
        fn drop(&mut self) {
            // If the interrupts were active before our `disable` call, then
            // re-enable them. Otherwise, keep them disabled.
            if self.0 {
                unsafe { enable() };
            }
        }
    }

    // Sampling before disabling is not atomic, but it cannot go stale: an
    // interrupt taken in the window returns through `sret`, which restores
    // `SIE` from `SPIE`.
    let _restore = Restore(sstatus::read().sie());
    disable();

    f()
}

/// Execute closure `f` with interrupts enabled for the current hart.
///
/// This function is designed to be run from within an interrupt handler to
/// allow for recursive interrupts.
///
/// The previous state is restored once `f` returns, including when it unwinds —
/// which matters here because a wasm trap unwinds straight through a handler.
///
/// # Safety
///
/// - The caller must ensure the remaining code is signal-safe.
/// - The interrupt flag must be cleared before calling this function, otherwise
///   the interrupt handler will be re-entered.
#[inline]
pub unsafe fn with_enabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    /// Restores the interrupt enable bit, `SPIE`, `SPP` and `SEPC` as they
    /// were.
    struct Restore {
        sstatus: sstatus::Sstatus,
        sepc: usize,
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            // If the interrupts were inactive before our `enable` call, then
            // re-disable them. Otherwise, keep them enabled.
            if !self.sstatus.sie() {
                disable();
            }

            // Restore SSTATUS.SPIE, SSTATUS.SPP, and SEPC
            if self.sstatus.spie() {
                unsafe { sstatus::set_spie() };
            }
            unsafe { sstatus::set_spp(self.sstatus.spp()) };
            sepc::set(self.sepc);
        }
    }

    let _restore = Restore {
        sstatus: sstatus::read(),
        sepc: sepc::read(),
    };

    // enable interrupts to allow nested interrupts
    unsafe { enable() };

    f()
}
