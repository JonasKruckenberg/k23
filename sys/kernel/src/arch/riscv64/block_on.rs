// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::arch::asm;

use anyhow::ensure;
use cpu_local::cpu_local;
use kasync::block_on::Notify;
use riscv::sbi;

use crate::state;

cpu_local! {
    static NOTIFY: Notify<RiscvPark> = Notify::new(RiscvPark {
        cpuid: state::cpu_local().id
    });
}

pub fn block_on<F: Future>(f: F) -> crate::Result<F::Output> {
    kasync::block_on::block_on(&*NOTIFY, f)
}

struct RiscvPark {
    cpuid: usize,
}

impl kasync::block_on::Park for RiscvPark {
    type Error = anyhow::Error;

    fn park(&self) -> Result<(), Self::Error> {
        let calling_cpuid = state::cpu_local().id;
        ensure!(self.cpuid == calling_cpuid);

        tracing::trace!("parking hart {calling_cpuid}");

        // Safety: wfi (wait for interrupt) halts the calling hart until an interrupt is received.
        // The calling hart will therefore not make any progress until woken by an IPI (from `unpark` below)
        // or through any other external interrupt.
        // We also need S-mode interrupts to be enabled on the calling hart (RISC-V Privileged §3.2.3), the HART init
        // procedure ensures this.
        unsafe { asm!("wfi", options(nomem, nostack, preserves_flags)) };

        tracing::trace!("hart {calling_cpuid} woke up");

        Ok(())
    }

    fn unpark(&self) -> Result<(), Self::Error> {
        sbi::ipi::send_ipi(1, self.cpuid)?;

        Ok(())
    }
}
