// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Process management host functions for WebAssembly modules

use crate::wasm::Linker;

/// Error codes
pub const ERRNO_SUCCESS: i32 = 0;

/// Register process host functions with the linker
pub fn register<T>(linker: &mut Linker<T>) -> crate::Result<()> {
    // proc_exit - Terminate the process
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "proc_exit",
        |exit_code: i32| -> i32 {
            tracing::info!("[WASM] proc_exit({})", exit_code);

            // TODO: Properly terminate the WASM instance
            // For now, just log and return the exit code
            // In a real implementation, this would terminate the WASM instance
            exit_code
        },
    )?;

    // environ_sizes_get - Get environment variable data sizes
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "environ_sizes_get",
        |_environ_count_ptr: i32, _environ_buf_size_ptr: i32| -> i32 {
            tracing::debug!("[WASM] environ_sizes_get()");

            // TODO: Write 0 to both pointers (no environment variables)
            // For now, return success with no env vars

            ERRNO_SUCCESS
        },
    )?;

    // environ_get - Get environment variable data
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "environ_get",
        |_environ_ptr: i32, _environ_buf_ptr: i32| -> i32 {
            tracing::debug!("[WASM] environ_get()");

            // No environment variables to return
            ERRNO_SUCCESS
        },
    )?;

    // args_sizes_get - Get command-line argument data sizes
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "args_sizes_get",
        |_argc_ptr: i32, _argv_buf_size_ptr: i32| -> i32 {
            tracing::debug!("[WASM] args_sizes_get()");

            // TODO: Write 0 to argc_ptr and 0 to argv_buf_size_ptr (no arguments)
            // For now, return success with no args

            ERRNO_SUCCESS
        },
    )?;

    // args_get - Get command-line argument data
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "args_get",
        |_argv_ptr: i32, _argv_buf_ptr: i32| -> i32 {
            tracing::debug!("[WASM] args_get()");

            // No arguments to return
            ERRNO_SUCCESS
        },
    )?;

    // random_get - Get random bytes
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "random_get",
        |_buf_ptr: i32, buf_len: i32| -> i32 {
            tracing::debug!("[WASM] random_get(len={})", buf_len);

            // TODO: Generate random bytes and write to buf_ptr
            // For now, just return success (buffer will contain uninitialized data)

            ERRNO_SUCCESS
        },
    )?;

    Ok(())
}
