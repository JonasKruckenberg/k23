use crate::runtime::codegen::translated_module::TranslatedModule;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_wasm::{DefinedFuncIndex, ModuleInternedTypeIndex, WasmSubType};

#[derive(Debug)]
pub struct CompiledModuleInfo<'wasm> {
    pub module: TranslatedModule<'wasm>,
    pub funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
    pub types: PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
}

#[derive(Debug)]
pub struct CompiledFunctionInfo {
    /// The [`FunctionLoc`] indicating the location of this function in the text
    /// section of the compilation artifact.
    pub wasm_func_loc: FunctionLoc,
    /// A trampoline for native callers (e.g. `Func::wrap`) calling into this function (if needed).
    pub native_to_wasm_trampoline: Option<FunctionLoc>,
}

/// Description of where a function is located in the text section of a
/// compiled image.
#[derive(Debug, Copy, Clone)]
pub struct FunctionLoc {
    /// The byte offset from the start of the text section where this
    /// function starts.
    pub start: u32,
    /// The byte length of this function's function body.
    pub length: u32,
}
