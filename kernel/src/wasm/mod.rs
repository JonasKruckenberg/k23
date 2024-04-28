use crate::kconfig;
use crate::wasm::func_env::FuncEnvironment;
use crate::wasm::module_env::ModuleEnvironment;
use crate::wasm::utils::value_type;
use core::mem;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::dominator_tree::DominatorTree;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir;
use cranelift_codegen::ir::AbiParam;
use cranelift_codegen::settings::Configurable;
use cranelift_wasm::wasmparser::{FuncValidator, FunctionBody, WasmModuleResources};
use cranelift_wasm::{FuncTranslator, WasmResult};

mod builtins;
mod func_env;
mod module;
mod module_env;
mod utils;
mod vmcontext;

/// Trap code used for debug assertions we emit in our JIT code.
const DEBUG_ASSERT_TRAP_CODE: u16 = u16::MAX;

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_WASMTIME_BUILTIN: u32 = 1;

/// Magic value for core Wasm VM contexts.
///
/// This is stored at the start of all `VMContext` structures.
pub const VMCONTEXT_MAGIC: u32 = u32::from_le_bytes(*b"CT23");

/// If this bit is set on a GC reference, then the GC reference is actually an
/// unboxed `i31`.
const I31_REF_DISCRIMINANT: u32 = 1;

/// WebAssembly page sizes are defined to be 64KiB.
pub const WASM_PAGE_SIZE: u32 = 0x10000;

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;

/// The size of the guard page before a linear memory in bytes, set pretty arbitrarily to 16KiB.
pub const MEMORY_GUARD_SIZE: u64 = 0x4000;

pub fn translate(module: &[u8]) -> WasmResult<()> {
    let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
    let mut b = cranelift_codegen::settings::builder();
    b.set("opt_level", "speed_and_size").unwrap();

    let isa = isa_builder
        .finish(cranelift_codegen::settings::Flags::new(b))
        .unwrap();

    let mut module_env = ModuleEnvironment::new();

    cranelift_wasm::translate_module(module, &mut module_env).unwrap();

    let (mut translation, types) = module_env.finish();
    let functions = mem::take(&mut translation.function_body_inputs);

    let mut func_translator = FuncTranslator::new();

    for (def_func_index, mut func) in functions {
        log::debug!("translating func {def_func_index:?}...");
        log::debug!("{translation:#?}");

        let sig_index =
            translation.module.functions[translation.module.func_index(def_func_index)].signature;
        let sig = types[sig_index].unwrap_func();

        let mut translated = ir::Function::new();
        translated.signature.params = sig
            .params()
            .iter()
            .map(|p| AbiParam::new(value_type(isa.as_ref(), *p)))
            .collect();
        translated.signature.returns = sig
            .returns()
            .iter()
            .map(|p| AbiParam::new(value_type(isa.as_ref(), *p)))
            .collect();

        let mut func_env = FuncEnvironment::new(isa.as_ref(), &translation);

        func_translator.translate_body(
            &mut func.validator,
            func.body,
            &mut translated,
            &mut func_env,
        )?;

        log::debug!("translated func {}", translated.display());

        let cfg = ControlFlowGraph::with_function(&translated);
        let domtree = DominatorTree::with_function(&translated, &cfg);

        let compiled = isa
            .compile_function(&translated, &domtree, false, &mut ControlPlane::default())
            .unwrap();
        let compiled = compiled.apply_params(&translated.params);

        // tval  0xffffffd7fffdc3e8

        // tdata 0xffffffd7ffffff0f..0xffffffd800000000
        // tbss  0xffffffd7fffff000..0xffffffd7ffffff0f
        // stack 0xffffffd7fffbf000..0xffffffd7fffff000
        // heap  0xffffffd7fdfbe000..0xffffffd7fffbe000
    }

    Ok(())
}
