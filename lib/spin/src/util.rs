// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

/// Runs `f` with interrupts masked on the calling hart, restoring the previous
/// state afterwards — including when `f` unwinds.
#[inline]
pub(crate) fn with_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    cfg_if::cfg_if! {
        if #[cfg(not(target_os = "none"))] {
            // A host process has no interrupts to mask. The inner spinlock
            // still provides the mutual exclusion the `Irq*` types promise;
            // only the re-entrancy guard is moot, since there is no handler to
            // be re-entered from.
            f()
        } else if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::interrupt::with_disabled(f)
        } else {
            compile_error!("unsupported target architecture")
        }
    }
}
