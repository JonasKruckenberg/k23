mod builtins;
mod func_env;
mod module;
mod module_env;
mod vmcontext;

use crate::wasm::func_env::FuncEnvironment;
use crate::wasm::module_env::ModuleEnvironment;
use alloc::vec::Vec;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::dominator_tree::DominatorTree;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir::types::{F32, F64, I32, I64, I8X16, R32, R64};
use cranelift_codegen::ir::{AbiParam, ArgumentPurpose, Function, Type};
use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::settings::Configurable;
use cranelift_wasm::{FuncIndex, FuncTranslator, TypeIndex, WasmHeapType, WasmResult, WasmValType};
use target_lexicon::{
    Aarch64Architecture, Architecture, BinaryFormat, Environment, OperatingSystem, Vendor,
};

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_WASMTIME_BUILTIN: u32 = 1;

/// WebAssembly page sizes are defined to be 64KiB.
pub const WASM_PAGE_SIZE: u32 = 0x10000;

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;

pub fn translate(module: &[u8]) -> WasmResult<()> {
    let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
    let mut b = cranelift_codegen::settings::builder();
    b.set("opt_level", "speed_and_size").unwrap();

    let isa = isa_builder
        .finish(cranelift_codegen::settings::Flags::new(b))
        .unwrap();

    let mut module_env = ModuleEnvironment::new();

    cranelift_wasm::translate_module(module, &mut module_env)?;

    let translation = module_env.finish();

    log::debug!("{:?} {:?}", translation.module, translation.types);

    assert_eq!(translation.module.types.len(), 1);
    let ty_idx = translation.module.types[TypeIndex::from_u32(0)];
    let f = translation.types[ty_idx].unwrap_func();
    assert_eq!(f.params().len(), 1);
    assert_eq!(f.returns().len(), 1);
    assert_eq!(f.non_i31_gc_ref_params_count(), 0);
    assert_eq!(f.non_i31_gc_ref_returns_count(), 0);

    assert_eq!(translation.module.functions.len(), 1);
    assert_eq!(
        translation.module.functions[FuncIndex::from_u32(0)].signature,
        ty_idx
    );

    assert_eq!(translation.module.table_plans.len(), 1);
    assert_eq!(translation.module.memory_plans.len(), 1);
    assert_eq!(translation.module.globals.len(), 1);
    assert_eq!(translation.module.exports.len(), 2);
    assert_eq!(translation.function_body_inputs.len(), 1);

    for (def_func_index, mut func_body_input) in translation.function_body_inputs {
        let sig_index =
            translation.module.functions[translation.module.func_index(def_func_index)].signature;
        let sig = translation.types[sig_index].unwrap_func();

        let mut translated = Function::new();
        // translated.signature.sp
        translated
            .signature
            .params
            .push(AbiParam::special(I64, ArgumentPurpose::VMContext));
        translated.signature.params.extend(
            sig.params()
                .iter()
                .map(|p| AbiParam::new(value_type(isa.as_ref(), *p))),
        );
        translated.signature.returns = sig
            .returns()
            .iter()
            .map(|p| AbiParam::new(value_type(isa.as_ref(), *p)))
            .collect();

        let mut func_env = FuncEnvironment::new(isa.as_ref(), &translation.module);
        let mut func_translator = FuncTranslator::new();

        func_translator.translate_body(
            &mut func_body_input.validator,
            func_body_input.body,
            &mut translated,
            &mut func_env,
        )?;

        let mut ctx = cranelift_codegen::Context::for_function(translated);
        ctx.optimize(isa.as_ref(), &mut ControlPlane::default())
            .unwrap();
        ctx.replace_redundant_loads().unwrap();
        ctx.verify_if(isa.as_ref()).unwrap();

        let mut out = Vec::new();
        let compiled = ctx
            .compile_and_emit(isa.as_ref(), &mut out, &mut ControlPlane::default())
            .unwrap();
    }

    todo!()
}

/// Returns the corresponding cranelift type for the provided wasm type.
pub fn value_type(isa: &dyn TargetIsa, ty: WasmValType) -> Type {
    match ty {
        WasmValType::I32 => I32,
        WasmValType::I64 => I64,
        WasmValType::F32 => F32,
        WasmValType::F64 => F64,
        WasmValType::V128 => I8X16,
        WasmValType::Ref(rt) => reference_type(rt.heap_type, isa.pointer_type()),
    }
}

/// Returns the reference type to use for the provided wasm type.
pub fn reference_type(wasm_ht: WasmHeapType, pointer_type: Type) -> Type {
    match wasm_ht {
        WasmHeapType::Func | WasmHeapType::ConcreteFunc(_) | WasmHeapType::NoFunc => pointer_type,
        WasmHeapType::Extern | WasmHeapType::Any | WasmHeapType::I31 | WasmHeapType::None => {
            match pointer_type {
                I32 => R32,
                I64 => R64,
                _ => panic!("unsupported pointer type"),
            }
        }
    }
}
