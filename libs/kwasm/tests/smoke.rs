#![feature(allocator_api)] // Copyright 2025. Jonas Kruckenberg

//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use kwasm::{ConstExprEvaluator, Engine, Linker, Mmap, Module, Store};
use smallvec::alloc;
use wasmparser::Validator;

#[tokio::test]
async fn main() {
    let engine = Engine::new();
    let mut validator = Validator::new();
    let mut linker = Linker::new(engine.clone());
    let mut const_eval = ConstExprEvaluator::default();

    let mut store = Store::new(engine.clone(), Box::new(alloc::alloc::Global), ());

    let module = Module::from_bytes(
        engine.clone(),
        &mut validator,
        mmap_os,
        include_bytes!("./fib.wasm"),
    )
    .unwrap();

    let instance = linker
        .instantiate(&mut store, &mut const_eval, &module)
        .unwrap();

    let func = instance.get_func(store.opaque_mut(), "fib").unwrap();

    let func = func.typed::<i32, i32>(store.opaque()).unwrap();

    let res = func.call(store.opaque_mut(), 8).await.unwrap();
    assert_eq!(res, 34);
}

fn mmap_os(bytes: Vec<u8>) -> kwasm::Result<Mmap> {
    todo!()
}
