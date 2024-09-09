#[derive(onlyerror::Error, Debug)]
pub enum CompileError {
    #[error("WebAssembly parsing failed {0}")]
    Parse(wasmparser::BinaryReaderError),
    #[error("WebAssembly to Cranelift IR translation failed {0}")]
    Translate(cranelift_wasm::WasmError),
    #[error("Cranelift IR to machine code compilation failed {0}")]
    Compile(cranelift_codegen::CodegenError),
}

impl From<cranelift_wasm::wasmparser::BinaryReaderError> for CompileError {
    fn from(error: cranelift_wasm::wasmparser::BinaryReaderError) -> Self {
        Self::Parse(error)
    }
}

impl<'a> From<cranelift_codegen::CompileError<'a>> for CompileError {
    fn from(error: cranelift_codegen::CompileError) -> Self {
        Self::Compile(error.inner)
    }
}
