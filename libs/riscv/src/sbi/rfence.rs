// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! RFENCE Extension

use super::{sbi_call, EID_RFNC};

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn sfence_vma(
    hart_mask: usize,
    hart_mask_base: usize,
    start_addr: usize,
    size: usize,
) -> super::Result<()> {
    sbi_call!(ext: EID_RFNC, func: 2, "a0": hart_mask, "a1": hart_mask_base, "a2": start_addr, "a3": size)?;
    Ok(())
}

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn sfence_vma_asid(
    hart_mask: usize,
    hart_mask_base: usize,
    start_addr: usize,
    size: usize,
    asid: u16,
) -> super::Result<()> {
    sbi_call!(ext: EID_RFNC, func: 2, "a0": hart_mask, "a1": hart_mask_base, "a2": start_addr, "a3": size, "a4": asid)?;
    Ok(())
}
