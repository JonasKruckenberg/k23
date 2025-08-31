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
        |fd: i32, iovs_ptr: i32, iovs_len: i32, nwritten_ptr: i32| -> i32 {
            // Validate parameters first
            if iovs_ptr < 0 || iovs_len < 0 || nwritten_ptr < 0 {
                return ERRNO_INVAL;
            }

            // Check if fd is valid: stdout (1), stderr (2), or reasonable file descriptors (3-63)
            // Invalid: stdin (0), negative values, or unreasonably high values
            if fd == FD_STDOUT || fd == FD_STDERR || (fd >= 3 && fd < 64) {
                // Valid fd - log and proceed
                if fd == FD_STDOUT || fd == FD_STDERR {
                    let prefix = if fd == FD_STDOUT { "stdout" } else { "stderr" };
                    tracing::debug!("[WASM {}] fd_write called with {} iovecs", prefix, iovs_len);
                    
                    // For demonstration, log that we would output data
                    // In a real implementation with memory access, we would read the IoVecs
                    // and output the actual data
                    tracing::info!("[WASM {}] Writing {} iovecs", prefix, iovs_len);
                } else {
                    tracing::debug!("[WASM] fd_write called for fd={} with {} iovecs", fd, iovs_len);
                }

                // TODO: When we have proper memory access:
                // 1. Read IoVec array from WASM memory at iovs_ptr
                // 2. For each IoVec, read the actual data buffer
                // 3. Output the data to console or file
                // 4. Write total bytes written to nwritten_ptr
                
                // For now, return success
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
        |fd: i32, iovs_ptr: i32, iovs_len: i32, nread_ptr: i32| -> i32 {
            // Can't read from stdout/stderr
            if fd == FD_STDOUT || fd == FD_STDERR {
                return ERRNO_BADF;
            }
            
            // Invalid fd
            if fd < 0 {
                return ERRNO_BADF;
            }

            // Validate parameters
            if iovs_ptr < 0 || iovs_len < 0 || nread_ptr < 0 {
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
            // For now, just return success
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
