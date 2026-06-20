// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

/// Disables interrupts and returns a guard that restores the previous state
/// on drop.
#[inline]
pub(crate) fn hold_interrupts() -> HeldInterrupts {
    // Safety: paired with the `release` in `HeldInterrupts::drop`.
    HeldInterrupts(unsafe { critical_section::acquire() })
}

/// An RAII guard that keeps interrupts disabled for as long as it is held.
pub(crate) struct HeldInterrupts(critical_section::RestoreState);

impl HeldInterrupts {
    /// Restores the previous interrupt state for the duration of `f`, then
    /// disables interrupts again.
    #[inline]
    pub(crate) fn with_released<F, U>(&mut self, f: F) -> U
    where
        F: FnOnce() -> U,
    {
        // Re-disable interrupts on the way out — including on unwind — refreshing
        // the saved state so this guard's own `Drop` stays balanced.
        struct Rearm<'a>(&'a mut critical_section::RestoreState);
        impl Drop for Rearm<'_> {
            fn drop(&mut self) {
                // Safety: pairs with the `release` below; the refreshed state is
                // released when the owning `HeldInterrupts` is dropped.
                *self.0 = unsafe { critical_section::acquire() };
            }
        }

        // Safety: paired with the re-acquire in `Rearm::drop`; restores the state
        // saved at construction for the duration of `f`.
        unsafe { critical_section::release(self.0) };
        let _rearm = Rearm(&mut self.0);
        f()
    }
}

impl Drop for HeldInterrupts {
    #[inline]
    fn drop(&mut self) {
        // Safety: restores the state saved by `HeldInterrupts::disable`, exactly once.
        unsafe { critical_section::release(self.0) };
    }
}
