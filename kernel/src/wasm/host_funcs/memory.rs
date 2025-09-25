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
    linker.func_wrap("wasi_snapshot_preview1", "memory_size", || -> i32 {
        // TODO: Get actual memory size from the caller's instance
        tracing::info!("[WASM] memory_size() called - returning 0 (stub)");
        0
    })?;

    // memory.grow - Grow memory by delta pages
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "memory_grow",
        |delta: i32| -> i32 {
            tracing::info!("[WASM] memory_grow({}) called - returning -1 (stub)", delta);
            -1
        },
    )?;

    Ok(())
}
