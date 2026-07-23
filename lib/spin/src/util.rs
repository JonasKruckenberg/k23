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

// this type MUST NOT be `Send` because toggling interrupts is fundamentally a
// per-hart operation
impl !Send for HeldInterrupts {}

impl Drop for HeldInterrupts {
    #[inline]
    fn drop(&mut self) {
        // Safety: restores the state saved by `HeldInterrupts::disable`, exactly once.
        unsafe { critical_section::release(self.0) };
    }
}
