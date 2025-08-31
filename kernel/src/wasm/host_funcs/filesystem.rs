// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Stub filesystem host functions for WebAssembly modules
//!
//! This module provides minimal filesystem functions that return fake data
//! to allow WASI programs to run without actually implementing a real filesystem.

use core::sync::atomic::{AtomicI32, Ordering};
use crate::wasm::Linker;

/// Error codes
pub const ERRNO_SUCCESS: i32 = 0;
pub const ERRNO_BADF: i32 = 8;     // Bad file descriptor
pub const ERRNO_EXIST: i32 = 17;   // File exists
pub const ERRNO_NOTDIR: i32 = 20;  // Not a directory
pub const ERRNO_ISDIR: i32 = 21;   // Is a directory
pub const ERRNO_INVAL: i32 = 28;   // Invalid argument
pub const ERRNO_NOENT: i32 = 44;   // No such file or directory
pub const ERRNO_NOSYS: i32 = 52;   // Function not implemented

/// File types
pub const FILETYPE_UNKNOWN: u8 = 0;
pub const FILETYPE_BLOCK_DEVICE: u8 = 1;
pub const FILETYPE_CHARACTER_DEVICE: u8 = 2;
pub const FILETYPE_DIRECTORY: u8 = 3;
pub const FILETYPE_REGULAR_FILE: u8 = 4;
pub const FILETYPE_SOCKET_DGRAM: u8 = 5;
pub const FILETYPE_SOCKET_STREAM: u8 = 6;
pub const FILETYPE_SYMBOLIC_LINK: u8 = 7;

/// Open flags
pub const OFLAGS_CREAT: u16 = 0x0001;
pub const OFLAGS_DIRECTORY: u16 = 0x0002;
pub const OFLAGS_EXCL: u16 = 0x0004;
pub const OFLAGS_TRUNC: u16 = 0x0008;

/// FD flags
pub const FDFLAGS_APPEND: u16 = 0x0001;
pub const FDFLAGS_DSYNC: u16 = 0x0002;
pub const FDFLAGS_NONBLOCK: u16 = 0x0004;
pub const FDFLAGS_RSYNC: u16 = 0x0008;
pub const FDFLAGS_SYNC: u16 = 0x0010;

/// Whence values for seek
pub const WHENCE_SET: i32 = 0;
pub const WHENCE_CUR: i32 = 1;
pub const WHENCE_END: i32 = 2;

/// Rights (simplified - stub returns all rights)
pub const RIGHTS_ALL: u64 = 0xFFFFFFFFFFFFFFFF;

/// Prestat tag
pub const PREOPENTYPE_DIR: u8 = 0;

/// Global next file descriptor counter
static NEXT_FD: AtomicI32 = AtomicI32::new(4); // Start at 4 (0-2 are stdio, 3 is preopen)

/// Filestat structure (WASI-compatible)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Filestat {
    pub dev: u64,
    pub ino: u64,
    pub filetype: u8,
    pub nlink: u64,
    pub size: u64,
    pub atim: u64,
    pub mtim: u64,
    pub ctim: u64,
}

impl Default for Filestat {
    fn default() -> Self {
        Self {
            dev: 0,
            ino: 1,
            filetype: FILETYPE_REGULAR_FILE,
            nlink: 1,
            size: 0,
            atim: 1700000000000000000,
            mtim: 1700000000000000000,
            ctim: 1700000000000000000,
        }
    }
}

/// Fdstat structure (WASI-compatible)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Fdstat {
    pub fs_filetype: u8,
    pub fs_flags: u16,
    pub _padding: [u8; 5],
    pub fs_rights_base: u64,
    pub fs_rights_inheriting: u64,
}

impl Default for Fdstat {
    fn default() -> Self {
        Self {
            fs_filetype: FILETYPE_REGULAR_FILE,
            fs_flags: 0,
            _padding: [0; 5],
            fs_rights_base: RIGHTS_ALL,
            fs_rights_inheriting: RIGHTS_ALL,
        }
    }
}

/// Prestat structure
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Prestat {
    pub tag: u8,
    pub u: PrestatU,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union PrestatU {
    pub dir: PrestatDir,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PrestatDir {
    pub pr_name_len: u32,
}

/// Directory entry
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Dirent {
    pub d_next: u64,    // Next cookie
    pub d_ino: u64,     // Inode
    pub d_namlen: u32,  // Name length
    pub d_type: u8,     // File type
    pub _padding: [u8; 3],
}

/// Register filesystem host functions
pub fn register<T>(linker: &mut Linker<T>) -> crate::Result<()> {
    // fd_prestat_get - Get preopened directory info
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_prestat_get",
        |fd: i32, prestat_ptr: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_prestat_get(fd={}, ptr={})", fd, prestat_ptr);
            
            // Only fd=3 is preopened (root directory)
            if fd != 3 {
                return ERRNO_BADF;
            }
            
            // TODO: Write prestat structure to memory
            // Structure should be:
            // - tag: u8 = 0 (PREOPENTYPE_DIR)
            // - padding: 3 bytes
            // - name_len: u32 = 1 (for "/")
            
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_prestat_dir_name - Get preopened directory path
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_prestat_dir_name",
        |fd: i32, path_ptr: i32, path_len: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_prestat_dir_name(fd={}, ptr={}, len={})", 
                fd, path_ptr, path_len);
            
            // Only fd=3 is preopened
            if fd != 3 {
                return ERRNO_BADF;
            }
            
            if path_len < 1 {
                return ERRNO_INVAL;
            }
            
            // TODO: Write "/" to the buffer at path_ptr
            
            ERRNO_SUCCESS
        },
    )?;
    
    // path_open - Open a file or directory
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "path_open",
        |dirfd: i32, 
         _dirflags: i32, 
         _path_ptr: i32, 
         _path_len: i32,
         oflags: i32,
         _fs_rights_base: i64,
         _fs_rights_inheriting: i64,
         _fdflags: i32,
         _fd_ptr: i32| -> i32 {
            tracing::debug!("[WASM FS] path_open(dirfd={}, path_ptr={}, path_len={}, oflags={})", 
                dirfd, _path_ptr, _path_len, oflags);
            
            // Generate a new file descriptor
            let new_fd = NEXT_FD.fetch_add(1, Ordering::SeqCst);
            
            // TODO: Write the new fd to memory at fd_ptr
            
            tracing::debug!("[WASM FS] path_open: returned fd={}", new_fd);
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_filestat_get - Get file statistics by file descriptor
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_filestat_get",
        |fd: i32, _filestat_ptr: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_filestat_get(fd={}, ptr={})", fd, _filestat_ptr);
            
            // Validate fd
            if fd < 0 {
                return ERRNO_BADF;
            }
            
            // TODO: Write filestat structure to memory
            // For stub, just return success
            
            ERRNO_SUCCESS
        },
    )?;
    
    // path_filestat_get - Get file statistics by path
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "path_filestat_get",
        |dirfd: i32, 
         _flags: i32,
         _path_ptr: i32, 
         _path_len: i32,
         _filestat_ptr: i32| -> i32 {
            tracing::debug!("[WASM FS] path_filestat_get(dirfd={}, path_ptr={}, path_len={})", 
                dirfd, _path_ptr, _path_len);
            
            // TODO: Write filestat structure to memory
            // For stub, just return success
            
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_fdstat_get - Get file descriptor status
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_fdstat_get",
        |fd: i32, _fdstat_ptr: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_fdstat_get(fd={}, ptr={})", fd, _fdstat_ptr);
            
            // Validate fd
            if fd < 0 {
                return ERRNO_BADF;
            }
            
            // TODO: Write fdstat structure to memory
            // For stub, just return success
            
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_readdir - Read directory entries
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_readdir",
        |fd: i32, 
         _buf_ptr: i32, 
         _buf_len: i32,
         cookie: i64,
         _bufused_ptr: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_readdir(fd={}, buf_ptr={}, buf_len={}, cookie={})", 
                fd, _buf_ptr, _buf_len, cookie);
            
            // TODO: Write 0 to bufused_ptr (empty directory)
            // For stub, just return success
            
            ERRNO_SUCCESS
        },
    )?;
    
    // path_create_directory - Create a directory
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "path_create_directory",
        |dirfd: i32, _path_ptr: i32, _path_len: i32| -> i32 {
            tracing::debug!("[WASM FS] path_create_directory(dirfd={}, path_ptr={}, path_len={})", 
                dirfd, _path_ptr, _path_len);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // path_remove_directory - Remove a directory
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "path_remove_directory",
        |dirfd: i32, _path_ptr: i32, _path_len: i32| -> i32 {
            tracing::debug!("[WASM FS] path_remove_directory(dirfd={}, path_ptr={}, path_len={})", 
                dirfd, _path_ptr, _path_len);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // path_unlink_file - Delete a file
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "path_unlink_file",
        |dirfd: i32, _path_ptr: i32, _path_len: i32| -> i32 {
            tracing::debug!("[WASM FS] path_unlink_file(dirfd={}, path_ptr={}, path_len={})", 
                dirfd, _path_ptr, _path_len);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // path_rename - Rename a file or directory
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "path_rename",
        |old_dirfd: i32, 
         _old_path_ptr: i32, 
         _old_path_len: i32,
         new_dirfd: i32,
         _new_path_ptr: i32,
         _new_path_len: i32| -> i32 {
            tracing::debug!("[WASM FS] path_rename(old_dirfd={}, new_dirfd={})", 
                old_dirfd, new_dirfd);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_filestat_set_times - Set file timestamps
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_filestat_set_times",
        |fd: i32, atim: i64, mtim: i64, fst_flags: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_filestat_set_times(fd={}, atim={}, mtim={}, flags={})", 
                fd, atim, mtim, fst_flags);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // path_filestat_set_times - Set path timestamps
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "path_filestat_set_times",
        |dirfd: i32, 
         _flags: i32,
         _path_ptr: i32, 
         _path_len: i32,
         _atim: i64,
         _mtim: i64,
         _fst_flags: i32| -> i32 {
            tracing::debug!("[WASM FS] path_filestat_set_times(dirfd={}, path_ptr={}, path_len={})", 
                dirfd, _path_ptr, _path_len);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_fdstat_set_flags - Set file descriptor flags
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_fdstat_set_flags",
        |fd: i32, flags: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_fdstat_set_flags(fd={}, flags={})", fd, flags);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_allocate - Allocate space for a file
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_allocate",
        |fd: i32, offset: i64, len: i64| -> i32 {
            tracing::debug!("[WASM FS] fd_allocate(fd={}, offset={}, len={})", fd, offset, len);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_datasync - Synchronize file data
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_datasync",
        |fd: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_datasync(fd={})", fd);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    // fd_sync - Synchronize file data and metadata
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_sync",
        |fd: i32| -> i32 {
            tracing::debug!("[WASM FS] fd_sync(fd={})", fd);
            
            // Stub: always succeed
            ERRNO_SUCCESS
        },
    )?;
    
    Ok(())
}