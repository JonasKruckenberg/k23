// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! I/O host functions for WebAssembly modules

use crate::wasm::Linker;
use crate::wasm::func::Caller;
use super::mem_access::MemoryAccessor;
use alloc::string::String;

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
    linker.func_wrap_with_memory(
        "wasi_snapshot_preview1",
        "fd_write",
        |mut caller: Caller<'_, T>, fd: i32, iovs_ptr: i32, iovs_len: i32, nwritten_ptr: i32| -> i32 {
            // Validate parameters first
            if iovs_ptr < 0 || iovs_len < 0 || nwritten_ptr < 0 {
                return ERRNO_INVAL;
            }

            // Check if fd is valid: stdout (1), stderr (2), or reasonable file descriptors (3-63)
            // Invalid: stdin (0), negative values, or unreasonably high values
            if fd == FD_STDOUT || fd == FD_STDERR || (fd >= 3 && fd < 64) {
                // Get memory accessor
                let mem = match MemoryAccessor::new(&mut caller) {
                    Some(m) => m,
                    None => return ERRNO_INVAL,
                };

                let mut total_written = 0u32;

                // Read IoVec array from WASM memory
                for i in 0..iovs_len {
                    let iov_offset = iovs_ptr as u32 + (i as u32 * 8); // Each IoVec is 8 bytes
                    
                    // Read IoVec structure
                    let buf_ptr = match unsafe { mem.read::<u32>(iov_offset) } {
                        Some(ptr) => ptr,
                        None => return ERRNO_INVAL,
                    };
                    
                    let buf_len = match unsafe { mem.read::<u32>(iov_offset + 4) } {
                        Some(len) => len,
                        None => return ERRNO_INVAL,
                    };

                    // Read actual data from WASM memory
                    if buf_len > 0 {
                        let data = match mem.read_bytes(buf_ptr, buf_len) {
                            Some(d) => d,
                            None => return ERRNO_INVAL,
                        };

                        // Output to console based on fd
                        if fd == FD_STDOUT || fd == FD_STDERR {
                            let output = String::from_utf8_lossy(&data);
                            // Use distinct targets and levels to differentiate streams
                            if fd == FD_STDOUT {
                                tracing::info!(target: "wasi::stdout", "{}", output);
                            } else {
                                tracing::error!(target: "wasi::stderr", "{}", output);
                            }
                        } else {
                            // For file descriptors, just count bytes (stub)
                            tracing::debug!("[WASM] fd_write to fd={}: {} bytes", fd, buf_len);
                        }

                        total_written += buf_len;
                    }
                }

                // Write bytes written count to nwritten_ptr
                if unsafe { mem.write::<u32>(nwritten_ptr as u32, &total_written) } {
                    ERRNO_SUCCESS
                } else {
                    ERRNO_INVAL
                }
            } else {
                // Invalid fd (includes stdin and any other invalid values)
                ERRNO_BADF
            }
        },
    )?;

    // fd_read - Read from a file descriptor
    linker.func_wrap_with_memory(
        "wasi_snapshot_preview1",
        "fd_read",
        |mut caller: Caller<'_, T>, fd: i32, iovs_ptr: i32, iovs_len: i32, nread_ptr: i32| -> i32 {
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

            // Get memory accessor
            let mem = match MemoryAccessor::new(&mut caller) {
                Some(m) => m,
                None => return ERRNO_INVAL,
            };

            // For stub implementation, always return EOF (0 bytes read)
            let total_read = 0u32;

            if fd == FD_STDIN {
                tracing::debug!("[WASM stdin] fd_read called, returning EOF");
            } else {
                tracing::debug!("[WASM] fd_read called for fd={}, returning EOF", fd);
            }

            // Write 0 to nread_ptr to indicate EOF
            if unsafe { mem.write::<u32>(nread_ptr as u32, &total_read) } {
                ERRNO_SUCCESS
            } else {
                ERRNO_INVAL
            }
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
    linker.func_wrap_with_memory(
        "wasi_snapshot_preview1",
        "fd_seek",
        |mut caller: Caller<'_, T>, fd: i32, offset: i64, whence: i32, newoffset_ptr: i32| -> i32 {
            tracing::debug!("[WASM] fd_seek({}, {}, {})", fd, offset, whence);
            
            if newoffset_ptr < 0 {
                return ERRNO_INVAL;
            }

            // Get memory accessor
            let mem = match MemoryAccessor::new(&mut caller) {
                Some(m) => m,
                None => return ERRNO_INVAL,
            };
            
            // For stub, just return the requested offset as new position
            let new_offset = offset as u64;
            
            // Write new position to newoffset_ptr
            if unsafe { mem.write::<u64>(newoffset_ptr as u32, &new_offset) } {
                ERRNO_SUCCESS
            } else {
                ERRNO_INVAL
            }
        },
    )?;

    Ok(())
}
