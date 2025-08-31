// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! I/O host functions for WebAssembly modules

use crate::wasm::Linker;

/// IoVec structure for scatter-gather I/O
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoVec {
    /// Pointer to buffer in WASM memory
    pub buf: u32,
    /// Length of buffer
    pub buf_len: u32,
}

/// File descriptor
pub const FD_STDIN: i32 = 0;
pub const FD_STDOUT: i32 = 1;
pub const FD_STDERR: i32 = 2;

/// Error code
pub const ERRNO_SUCCESS: i32 = 0;
pub const ERRNO_BADF: i32 = 8; // Bad file descriptor
pub const ERRNO_INVAL: i32 = 28; // Invalid argument
pub const ERRNO_NOSYS: i32 = 52; // Function not implemented

pub fn register<T>(linker: &mut Linker<T>) -> crate::Result<()> {
    // fd_write - Write to a file descriptor
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_write",
        |fd: i32, _iovs_ptr: i32, iovs_len: i32, _nwritten_ptr: i32| -> i32 {
            // Validate parameters first
            if _iovs_ptr < 0 || iovs_len < 0 || _nwritten_ptr < 0 {
                return ERRNO_INVAL;
            }

            // Check if fd is valid: stdout (1), stderr (2), or reasonable file descriptors (3-63)
            // Invalid: stdin (0), negative values, or unreasonably high values
            if fd == FD_STDOUT || fd == FD_STDERR || (fd >= 3 && fd < 64) {
                // Valid fd - log and proceed
                if fd == FD_STDOUT || fd == FD_STDERR {
                    let prefix = if fd == FD_STDOUT { "stdout" } else { "stderr" };
                    tracing::debug!("[WASM {}] fd_write called with {} iovecs", prefix, iovs_len);
                } else {
                    tracing::debug!("[WASM] fd_write called for fd={} with {} iovecs", fd, iovs_len);
                }

                // TODO: Read IoVec array from WASM memory
                // TODO: Read actual data from WASM memory
                // TODO: Write bytes written count to nwritten_ptr

                // Return success for valid fds
                ERRNO_SUCCESS
            } else {
                // Invalid fd (includes stdin and any other invalid values)
                ERRNO_BADF
            }
        },
    )?;

    // fd_read - Read from a file descriptor
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_read",
        |fd: i32, _iovs_ptr: i32, _iovs_len: i32, _nread_ptr: i32| -> i32 {
            // Can't read from stdout/stderr
            if fd == FD_STDOUT || fd == FD_STDERR {
                return ERRNO_BADF;
            }
            
            // Invalid fd
            if fd < 0 {
                return ERRNO_BADF;
            }

            // Validate parameters
            if _iovs_ptr < 0 || _iovs_len < 0 || _nread_ptr < 0 {
                return ERRNO_INVAL;
            }

            // TODO: Implement actual reading from console
            // For now, return 0 bytes read (EOF)
            if fd == FD_STDIN {
                tracing::debug!("[WASM stdin] fd_read called");
            } else {
                tracing::debug!("[WASM] fd_read called for fd={}", fd);
            }
            // TODO: Write 0 to nread_ptr to indicate EOF
            ERRNO_SUCCESS
        },
    )?;

    // fd_close - Close a file descriptor
    linker.func_wrap("wasi_snapshot_preview1", "fd_close", |fd: i32| -> i32 {
        tracing::debug!("[WASM] fd_close({})", fd);

        // Can't close standard streams
        if fd == FD_STDIN || fd == FD_STDOUT || fd == FD_STDERR {
            return ERRNO_BADF;
        }
        
        // For file descriptors, just return success (stub)
        if fd >= 3 {
            return ERRNO_SUCCESS;
        }
        
        ERRNO_BADF
    })?;

    // fd_seek - Seek in a file
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_seek",
        |fd: i32, offset: i64, whence: i32, _newoffset_ptr: i32| -> i32 {
            tracing::debug!("[WASM] fd_seek({}, {}, {})", fd, offset, whence);
            
            // For stub, just return success
            // TODO: Write new position to newoffset_ptr
            ERRNO_SUCCESS
        },
    )?;

    Ok(())
}
