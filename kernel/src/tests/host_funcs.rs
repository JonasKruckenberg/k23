// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Tests for host functions

use crate::wasm::{Engine, Linker, Module, Store};
use crate::wasm::host_funcs;
use crate::wasm::PlaceholderAllocatorDontUse;

#[ktest::test]
async fn test_host_functions_registration() {
    // Create engine and linker
    let engine = Engine::default();
    let mut linker: Linker<()> = Linker::new(&engine);
    
    // Register all host functions
    host_funcs::register_host_functions(&mut linker).expect("Failed to register host functions");
    
    // Test that we can also register test functions
    host_funcs::register_test_functions(&mut linker).expect("Failed to register test functions");
}

#[ktest::test]
async fn test_wasi_hello_world() {
    // This test would require a WASM module that uses WASI functions
    // For now, we just test that the functions can be registered
    
    let engine = Engine::default();
    let mut linker: Linker<()> = Linker::new(&engine);
    let mut store = Store::new(&engine, &PlaceholderAllocatorDontUse, ());
    
    // Register host functions
    host_funcs::register_host_functions(&mut linker).expect("Failed to register host functions");
    
    // We would need a WASM module here that imports WASI functions
    // For example, a simple program that calls fd_write to print "Hello, World!"
    
    // TODO: Add actual WASM module test once we have a way to compile test modules
}