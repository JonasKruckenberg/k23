mod builtins;
mod compile;
mod engine;
mod errors;
mod instance;
mod instantiate;
mod linker;
mod module;
mod translate;
mod utils;
mod vmcontext;

use crate::rt::instantiate::Store;
use crate::rt::linker::Linker;
use crate::rt::module::Module;
pub use builtins::{BuiltinFunctionIndex, BuiltinFunctionSignatures, BuiltinFunctions};
pub use compile::build_module;
use cranelift_codegen::settings::Configurable;
pub use engine::Engine;
pub use errors::CompileError;
pub use vmcontext::{
    FuncRefIndex, VMContext, VMContextOffsets, VMFuncRef, VMGlobalDefinition, VMMemoryDefinition,
    VMTableDefinition, VMCONTEXT_MAGIC,
};

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_WASM_BUILTIN: u32 = 1;

/// WebAssembly page sizes are defined to be 64KiB.
pub const WASM_PAGE_SIZE: u32 = 0x10000;

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;

/// Trap code used for debug assertions we emit in our JIT code.
pub const DEBUG_ASSERT_TRAP_CODE: u16 = u16::MAX;

pub fn test() {
    let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
    let mut b = cranelift_codegen::settings::builder();
    b.set("opt_level", "speed_and_size").unwrap();

    let target_isa = isa_builder
        .finish(cranelift_codegen::settings::Flags::new(b))
        .unwrap();

    let engine = Engine::new(target_isa);
    let wasm = include_bytes!("../../tests/fib-wasm.wasm");

    let mut store = Store::new();
    let linker = Linker::new();

    let module = Module::from_bytes(&engine, &mut store, wasm);

    let instance = linker.instantiate(&mut store, &engine, module);
    log::debug!("{store:#?}");

    let func = instance.get_func(&mut store, "fib").unwrap();
    log::debug!("{func:?}");

    func.call(&mut store);
}
