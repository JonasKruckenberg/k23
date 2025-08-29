// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Time and clock host functions for WebAssembly modules

use crate::wasm::Linker;

/// Clock IDs (WASI-compatible)
pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;
pub const CLOCK_PROCESS_CPUTIME: i32 = 2;
pub const CLOCK_THREAD_CPUTIME: i32 = 3;

/// Error codes
pub const ERRNO_SUCCESS: i32 = 0;
pub const ERRNO_INVAL: i32 = 28; // Invalid argument
pub const ERRNO_NOSYS: i32 = 52; // Function not implemented

/// Register time host functions with the linker
pub fn register<T>(linker: &mut Linker<T>) -> crate::Result<()> {
    // clock_time_get - Get time value of a clock
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "clock_time_get",
        |clock_id: i32, _precision: i64, _time_ptr: i32| -> i32 {
            tracing::debug!(
                "[WASM] clock_time_get(clock_id={}, precision={})",
                clock_id,
                _precision
            );

            // Validate clock ID
            match clock_id {
                CLOCK_REALTIME | CLOCK_MONOTONIC => {
                    // TODO: Get actual time from kernel clock
                    // TODO: Write time to time_ptr in WASM memory

                    // For now, just return success
                    ERRNO_SUCCESS
                }
                CLOCK_PROCESS_CPUTIME | CLOCK_THREAD_CPUTIME => {
                    // CPU time clocks not supported yet
                    ERRNO_NOSYS
                }
                _ => {
                    // Invalid clock ID
                    ERRNO_INVAL
                }
            }
        },
    )?;

    // clock_res_get - Get resolution of a clock
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "clock_res_get",
        |clock_id: i32, _resolution_ptr: i32| -> i32 {
            tracing::debug!("[WASM] clock_res_get(clock_id={})", clock_id);

            // Validate clock ID
            match clock_id {
                CLOCK_REALTIME | CLOCK_MONOTONIC => {
                    // TODO: Write clock resolution to resolution_ptr
                    // For now, assume 1 nanosecond resolution

                    ERRNO_SUCCESS
                }
                CLOCK_PROCESS_CPUTIME | CLOCK_THREAD_CPUTIME => {
                    // CPU time clocks not supported yet
                    ERRNO_NOSYS
                }
                _ => {
                    // Invalid clock ID
                    ERRNO_INVAL
                }
            }
        },
    )?;

    Ok(())
}
