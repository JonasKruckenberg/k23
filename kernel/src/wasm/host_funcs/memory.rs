// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Memory management host functions for WebAssembly modules

use crate::wasm::Linker;

/// Register memory host functions with the linker
pub fn register<T>(linker: &mut Linker<T>) -> crate::Result<()> {
    // memory.size - Get current memory size in pages
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "memory_size",
        || -> i32 {
            // TODO: Get actual memory size from the caller's instance
            tracing::debug!("[WASM] memory_size() called");
            
            // Return a placeholder value for now (64 pages = 4MB)
            64
        }
    )?;
    
    // memory.grow - Grow memory by delta pages
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "memory_grow",
        |delta: i32| -> i32 {
            tracing::debug!("[WASM] memory_grow({}) called", delta);
            
            if delta < 0 {
                // Cannot shrink memory
                return -1;
            }
            
            // TODO: Actually grow the memory of the caller's instance
            // For now, return -1 to indicate failure
            -1
        }
    )?;
    
    Ok(())
}