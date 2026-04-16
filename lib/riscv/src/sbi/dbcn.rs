// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Debug Console Extension

use super::{EID_DBCN, sbi_call};

/// # Errors
///
/// Returns an error if the SBI call fails.
#[inline]
pub fn debug_console_write(
    num_bytes: usize,
    base_addr_lo: usize,
    base_addr_hi: usize,
) -> super::Result<usize> {
    let bytes_written =
        sbi_call!(ext: EID_DBCN, func: 0, "a0": num_bytes, "a1": base_addr_lo, "a2": base_addr_hi)?;

    Ok(bytes_written)
}
