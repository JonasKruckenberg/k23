// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Host-imposed memory limits passed to `Linker::instantiate_with_limits`.

use alloc::format;
use alloc::vec::Vec;

use wasmparser::Validator;
use wast::Wat;
use wast::core::EncodeOptions;
use wast::parser::{self, ParseBuffer};

use crate::wasm::{
    ConstExprEvaluator, Engine, Extern, InstanceLimits, Linker, Memory, Module,
    PlaceholderAllocatorDontUse, Store,
};

/// One default WebAssembly page.
const PAGE: usize = 0x10000;

struct Harness {
    engine: Engine,
    store: Store<()>,
    linker: Linker<()>,
    const_eval: ConstExprEvaluator,
    validator: Validator,
}

impl Harness {
    fn new() -> Self {
        let engine = Engine::default();
        Self {
            linker: Linker::new(&engine),
            store: Store::new(&engine, &PlaceholderAllocatorDontUse, ()),
            const_eval: ConstExprEvaluator::default(),
            validator: Validator::new(),
            engine,
        }
    }

    fn instantiate(&mut self, wat: &str, max_pages: usize) -> crate::Result<Memory> {
        let bytes = encode(wat)?;
        let module = Module::from_bytes(&self.engine, &mut self.validator, &bytes)?;
        // Deliberately leak the module. A successful instantiation keeps it
        // alive through the (never deallocated) instance anyway; the failing
        // case below would otherwise drop it, and dropping a compiled module
        // unmaps its JIT code, which currently corrupts later kernel traps.
        core::mem::forget(module.clone());
        let limits = InstanceLimits {
            max_memory_bytes: Some(max_pages * PAGE),
        };
        let instance = self.linker.instantiate_with_limits(
            &mut self.store,
            &mut self.const_eval,
            &module,
            limits,
        )?;
        Ok(instance
            .get_export(&mut self.store, "mem")
            .and_then(Extern::into_memory)
            .expect("module exports a memory named `mem`"))
    }
}

fn encode(wat: &str) -> crate::Result<Vec<u8>> {
    let buf = ParseBuffer::new(wat)?;
    let mut wat = parser::parse::<Wat>(&buf)?;
    Ok(EncodeOptions::default().encode_wat(&mut wat)?)
}

#[test::test]
async fn host_cap_is_enforced() {
    let mut h = Harness::new();

    // `(memory 1 10)` capped at four pages: the cap wins over the declared
    // maximum.
    let mem = h
        .instantiate(r#"(module (memory (export "mem") 1 10))"#, 4)
        .unwrap();
    assert_eq!(mem.maximum_byte_size(&mut h.store), Some(4 * PAGE));

    // `(memory 2 10)` needs two pages, the cap allows one: instantiation
    // fails instead of panicking or clamping.
    let err = h
        .instantiate(r#"(module (memory (export "mem") 2 10))"#, 1)
        .expect_err("instantiation must fail when the minimum exceeds the cap");
    assert!(
        format!("{err:#}").contains("exceeds host limit"),
        "unexpected error: {err:#}"
    );
}
