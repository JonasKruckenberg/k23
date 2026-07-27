// Copyright 2023-Present Jonas Kruckenberg
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

use crate::cpu::{self, LogicalCpuId};
use crate::state;

cpu_local! {
    static NOTIFY: Notify<RiscvPark> = Notify::new(RiscvPark::for_current_cpu());
}

pub fn block_on<F: Future>(f: F) -> crate::Result<F::Output> {
    kasync::block_on::block_on(&*NOTIFY, f)
}

struct RiscvPark {
    cpu: LogicalCpuId,
    /// The hart `cpu` stands for. Cached rather than looked up, because
    /// `unpark` runs from other CPUs and must not reach into global state.
    hartid: usize,
}

impl RiscvPark {
    fn for_current_cpu() -> Self {
        let cpu = cpu::current();
        Self {
            cpu,
            hartid: state::global().cpus.hartid(cpu),
        }
    }
}

impl kasync::block_on::Park for RiscvPark {
    type Error = anyhow::Error;

    fn park(&self) -> Result<(), Self::Error> {
        let calling_cpu = cpu::current();
        ensure!(self.cpu == calling_cpu);

        let calling_cpuid = self.hartid;
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
        // The mask is relative to `hart_mask_base`, so this is "the one hart at
        // base" — the hardware id, not the `LogicalCpuId`.
        sbi::ipi::send_ipi(1, self.hartid)?;

        Ok(())
    }
}
