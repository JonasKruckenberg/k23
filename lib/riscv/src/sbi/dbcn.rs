// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Debug Console Extension

use super::{EID_DBCN, sbi_call};

/// Write bytes to the debug console from input memory.
///
/// The `num_bytes` parameter specifies the number of bytes in the input memory.
/// The physical base address of the input memory is represented by two usize
/// bits wide parameters. The `base_addr_lo` parameter specifies the lower XLEN
/// bits and the `base_addr_hi` parameter specifies the upper XLEN bits
/// of the input memory physical base address.
///
/// This is a non-blocking SBI call and it may do partial/no writes if the debug
/// console is not able to accept more bytes.
///
/// Returns the number of bytes written.
///
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
