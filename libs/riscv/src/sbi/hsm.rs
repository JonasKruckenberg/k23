// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Hart State Management Extension

use super::{sbi_call, EID_HSM};

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn start_hart(hartid: usize, start_address: usize, opaque: usize) -> super::Result<()> {
    sbi_call!(ext: EID_HSM, func: 0, "a0": hartid, "a1": start_address, "a2": opaque)?;
    Ok(())
}
