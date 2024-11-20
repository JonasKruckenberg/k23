#[derive(onlyerror::Error, Debug)]
pub enum CompileError {
    #[error("Translation error: {0}")]
    Translate(#[from] TranslationError),
    #[error("Cranelift IR to machine code compilation failed {0}")]
    Compile(cranelift_codegen::CodegenError),
}
impl From<cranelift_codegen::CompileError<'_>> for CompileError {
    fn from(error: cranelift_codegen::CompileError) -> Self {
        Self::Compile(error.inner)
    }
}

#[derive(onlyerror::Error, Debug)]
pub enum TranslationError {
    #[error("WebAssembly parsing failed {0}")]
    Parse(wasmparser::BinaryReaderError),
    #[error("WebAssembly to Cranelift IR translation failed {0}")]
    Translate(cranelift_wasm::WasmError),
}

impl From<wasmparser::BinaryReaderError> for TranslationError {
    fn from(error: wasmparser::BinaryReaderError) -> Self {
        Self::Parse(error)
    }
}

impl From<cranelift_wasm::WasmError> for TranslationError {
    fn from(error: cranelift_wasm::WasmError) -> Self {
        Self::Translate(error)
    }
}
