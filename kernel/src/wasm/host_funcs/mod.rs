// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Host functions for WebAssembly modules
//!
//! This module provides system-level host functions that WASM modules can import
//! to interact with the kernel. These functions provide basic I/O, memory, time,
//! and process management capabilities.

pub mod io;
pub mod memory;
pub mod process;
pub mod time;

use crate::wasm::{Linker, Store};

/// Register all standard host functions with the provided linker.
///
/// This registers functions under the "wasi_snapshot_preview1" module name
/// to provide basic WASI compatibility.
pub fn register_host_functions<T>(linker: &mut Linker<T>) -> crate::Result<()> {
    // Register I/O functions
    io::register(linker)?;

    // Register memory functions
    memory::register(linker)?;

    // Register time functions
    time::register(linker)?;

    // Register process functions
    process::register(linker)?;

    Ok(())
}

/// Register minimal host functions for testing.
///
/// This registers a smaller subset of functions under the "k23" module name
/// for use in tests and development.
pub fn register_test_functions<T>(linker: &mut Linker<T>) -> crate::Result<()> {
    // Register basic console output for testing
    linker.func_wrap("k23", "print", || {
        tracing::info!("[WASM] print() called");
    })?;

    linker.func_wrap("k23", "print_str", |ptr: i32, len: i32| {
        tracing::info!("[WASM] print_str(ptr={}, len={})", ptr, len);
        // TODO: Read string from WASM memory and print it
    })?;

    Ok(())
}
