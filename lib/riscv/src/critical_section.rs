// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use critical_section::{Impl, RawRestoreState, set_impl};

use crate::interrupt;

struct SingleHartCriticalSection;
set_impl!(SingleHartCriticalSection);

// Safety: we return the `RawRestoreState` that we expect callers to pass to `release`.
// all other contract invariants must be upheld by the caller.
unsafe impl Impl for SingleHartCriticalSection {
    unsafe fn acquire() -> RawRestoreState {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                let mut sstatus: usize;

                // Safety: inline assembly
                // NB: bitmask to disable SIE = sstatus bit 1
                unsafe { core::arch::asm!("csrrci {}, sstatus, 0b0010", out(reg) sstatus) };

                // Safety: `SStatus` is `repr(transparent)` over usize and can deal with arbitrary
                // bit patters.
                unsafe { core::mem::transmute::<_, crate::register::sstatus::Sstatus>(sstatus).sie() }
            } else {
                unimplemented!()
            }
        }
    }

    unsafe fn release(was_active: RawRestoreState) {
        // Only re-enable interrupts if they were enabled before the critical section.
        if was_active {
            // Safety: ensured by the caller
            unsafe { interrupt::enable() }
        }
    }
}
