#![allow(unused)]

mod builtins;
mod compiler;
mod func_env;
mod module;
mod module_env;
mod vmcontext;

use crate::wasm::compiler::Compiler;
use crate::wasm::func_env::FuncEnvironment;
use crate::wasm::module_env::ModuleEnvironment;
use alloc::vec::Vec;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::types::{F32, F64, I32, I64, I8X16, R32, R64};
use cranelift_codegen::ir::{AbiParam, ArgumentPurpose, Function, Signature, Type};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_codegen::settings::Configurable;
use cranelift_wasm::{
    FuncIndex, FuncTranslator, TypeIndex, WasmFuncType, WasmHeapType, WasmResult, WasmValType,
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

/// A position within an original source file,
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilePos(u32);

impl Default for FilePos {
    fn default() -> FilePos {
        FilePos(u32::MAX)
    }
}

pub fn translate(module: &[u8]) -> WasmResult<()> {
    let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
    let mut b = cranelift_codegen::settings::builder();
    b.set("opt_level", "speed_and_size").unwrap();

    let target_isa = isa_builder
        .finish(cranelift_codegen::settings::Flags::new(b))
        .unwrap();

    let compiler = Compiler::new(target_isa);

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

    for (def_func_index, input) in translation.function_body_inputs {
        let compiled = compiler.compile_function(
            &translation.module,
            &translation.types,
            def_func_index,
            input,
        );

        log::debug!("Func {def_func_index:?} relocations:");
        for reloc in compiled.relocations() {
            log::debug!("{reloc:?}");
        }
    }

    Ok(())
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

fn wasm_call_signature(target_isa: &dyn TargetIsa, wasm_func_ty: &WasmFuncType) -> Signature {
    let mut sig = Signature::new(CallConv::Fast);

    // Add the caller/callee `vmctx` parameters.
    sig.params.push(AbiParam::special(
        target_isa.pointer_type(),
        ArgumentPurpose::VMContext,
    ));

    let cvt = |ty: &WasmValType| AbiParam::new(value_type(target_isa, *ty));
    sig.params.extend(wasm_func_ty.params().iter().map(&cvt));
    sig.returns.extend(wasm_func_ty.returns().iter().map(&cvt));
    sig
}
