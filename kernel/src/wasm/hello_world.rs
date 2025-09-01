// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Hello World WASM module for testing WASI integration

use crate::wasm::{Engine, Linker, Module, Store};
use crate::wasm::host_funcs;
use crate::wasm::vm::{ConstExprEvaluator, instance_alloc::PlaceholderAllocatorDontUse};
use wasmparser::Validator;
use wast::parser::ParseBuffer;

/// The Hello World WAT module source
const HELLO_WORLD_WAT: &str = r#"
(module
    ;; Import fd_write from WASI
    (import "wasi_snapshot_preview1" "fd_write" 
        (func $fd_write (param i32 i32 i32 i32) (result i32)))
    
    ;; Memory with at least 1 page
    (memory (export "memory") 1)
    
    ;; Data segment with our message
    (data (i32.const 8) "Hello World from WASI!\n")
    
    ;; IoVec structure at offset 0
    ;; Points to our string at offset 8, length 23
    (data (i32.const 0) "\08\00\00\00\17\00\00\00")
    
    ;; Start function
    (func $main (export "_start")
        ;; Call fd_write(stdout=1, iovs=0, iovs_len=1, nwritten=100)
        i32.const 1      ;; fd: stdout
        i32.const 0      ;; iovs: pointer to IoVec array
        i32.const 1      ;; iovs_len: we have 1 IoVec
        i32.const 100    ;; nwritten: where to store bytes written
        call $fd_write
        drop             ;; ignore return value
    )
)
"#;

/// Run the Hello World WASM module
pub fn run() -> Result<(), &'static str> {
    tracing::debug!("Running Hello World WASM module");
    
    // Parse WAT to WASM
    let buf = ParseBuffer::new(HELLO_WORLD_WAT)
        .map_err(|_| "Failed to create parse buffer")?;
    let mut wat = wast::parser::parse::<wast::Wat>(&buf)
        .map_err(|_| "Failed to parse WAT")?;
    
    // Convert to WASM bytes
    let wasm_bytes = wat.encode()
        .map_err(|_| "Failed to encode WAT to WASM")?;
    
    // Create engine
    let engine = Engine::default();
    
    // Create allocator and store
    let alloc = &PlaceholderAllocatorDontUse as &(dyn crate::wasm::vm::InstanceAllocator + Send + Sync);
    let mut store = Store::new(&engine, alloc, ());
    
    // Create validator and module from WASM bytes
    let mut validator = Validator::new();
    let module = Module::from_bytes(&engine, &mut validator, &wasm_bytes)
        .map_err(|_| "Failed to create module")?;
    
    // Create linker and register host functions
    let mut linker = Linker::new(&engine);
    host_funcs::register_host_functions(&mut linker)
        .map_err(|_| "Failed to register host functions")?;
    
    // Create const expression evaluator
    let mut const_eval = ConstExprEvaluator::default();
    
    // Instantiate module
    let instance = linker.instantiate(&mut store, &mut const_eval, &module)
        .map_err(|_| "Failed to instantiate module")?;
    
    // Get and call _start function
    let start = instance
        .get_func(&mut store, "_start")
        .ok_or("No _start function found")?;
    
    // Call the function
    start.call(&mut store, &[], &mut [])
        .map_err(|_| "Failed to call _start")?;
    
    tracing::debug!("Hello World WASM module completed successfully");
    Ok(())
}