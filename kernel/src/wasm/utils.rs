use cranelift_codegen::ir;
use cranelift_codegen::ir::types::{F32, F64, I32, I64, I8X16, R32, R64};
use cranelift_codegen::isa::TargetIsa;
use cranelift_wasm::{WasmHeapType, WasmValType};

/// Returns the corresponding cranelift type for the provided wasm type.
pub fn value_type(isa: &dyn TargetIsa, ty: WasmValType) -> ir::types::Type {
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
pub fn reference_type(wasm_ht: WasmHeapType, pointer_type: ir::Type) -> ir::Type {
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
