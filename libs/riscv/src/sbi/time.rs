// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Timer Extension

use super::{EID_TIME, sbi_call};

/// # Errors
///
/// Returns an error if the SBI call fails.
///
/// # Panics
///
/// Panics if the conversion from `u64` to `usize` fails.
#[inline]
pub fn set_timer(stime_value: u64) -> super::Result<()> {
    let stime_value = usize::try_from(stime_value).unwrap();
    sbi_call!(ext: EID_TIME, func: 0, "a0": stime_value, "a1": 0_usize)?;

    Ok(())
}
