// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! IPI Extension

use super::{sbi_call, EID_IPI};

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn send_ipi(hart_mask: usize, hart_mask_base: usize) -> super::Result<()> {
    sbi_call!(ext: EID_IPI, func: 0, "a0": hart_mask, "a1": hart_mask_base)?;

    Ok(())
}
