// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Timer Extension

use super::{sbi_call, EID_TIME};

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn set_timer(stime_value: u64) -> super::Result<()> {
    sbi_call!(ext: EID_TIME, func: 0, "a0": usize::try_from(stime_value)?, "a1": 0)?;

    Ok(())
}
