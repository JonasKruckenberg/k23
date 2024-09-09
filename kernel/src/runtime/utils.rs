use cranelift_codegen::ir::types::{F32, F64, I32, I64, I8X16};
use cranelift_codegen::ir::{AbiParam, ArgumentPurpose, Signature, Type};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_wasm::{WasmFuncType, WasmHeapTopType, WasmHeapType, WasmValType};

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
    match wasm_ht.top() {
        WasmHeapTopType::Func => pointer_type,
        WasmHeapTopType::Any |  WasmHeapTopType::Extern => I32
    }
}

fn blank_sig(isa: &dyn TargetIsa, call_conv: CallConv) -> Signature {
    let pointer_type = isa.pointer_type();
    let mut sig = Signature::new(call_conv);

    // Add the caller/callee `vmctx` parameters.
    sig.params
        .push(AbiParam::special(pointer_type, ArgumentPurpose::VMContext));

    sig
}

pub fn wasm_call_signature(target_isa: &dyn TargetIsa, wasm_func_ty: &WasmFuncType) -> Signature {
    let mut sig = blank_sig(target_isa, CallConv::Fast);

    let cvt = |ty: &WasmValType| AbiParam::new(value_type(target_isa, *ty));
    sig.params.extend(wasm_func_ty.params().iter().map(&cvt));
    sig.returns.extend(wasm_func_ty.returns().iter().map(&cvt));

    sig
}

#[allow(unused)]
pub fn native_call_signature(target_isa: &dyn TargetIsa, wasm_func_ty: &WasmFuncType) -> Signature {
    let mut sig = blank_sig(target_isa, CallConv::triple_default(target_isa.triple()));

    let cvt = |ty: &WasmValType| AbiParam::new(value_type(target_isa, *ty));
    sig.params.extend(wasm_func_ty.params().iter().map(&cvt));
    sig.returns.extend(wasm_func_ty.returns().iter().map(&cvt));

    sig
}
