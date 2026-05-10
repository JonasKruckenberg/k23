// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! IPI Extension

use super::{EID_IPI, sbi_call};

/// Send an IPI to harts selected by `hart_mask`, where bit `k` targets hartid
/// `hart_mask_base + k`. Pass `hart_mask_base = -1` to broadcast to all harts.
///
/// The [SBI spec][spec] states the hart_mask_base is the "starting hartid". In
/// practice [OpenSBI `sbi_ipi_send_many`][opensbi] and [Linux `__sbi_send_ipi_v02`][linux]
/// treat this as meaning "the bit position(s) in hart_mask PLUS the hart_mask_base" is the final HART ID.
///
/// [spec]: https://github.com/riscv-non-isa/riscv-sbi-doc/blob/master/src/ext-ipi.adoc
/// [opensbi]: https://github.com/riscv-software-src/opensbi/blob/master/lib/sbi/sbi_ipi.c
/// [linux]: https://github.com/torvalds/linux/blob/master/arch/riscv/kernel/sbi.c
///
/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn send_ipi(hart_mask: usize, hart_mask_base: usize) -> super::Result<()> {
    sbi_call!(ext: EID_IPI, func: 0, "a0": hart_mask, "a1": hart_mask_base)?;

    Ok(())
}
