#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("failed to parse WASM {0}")]
    WasmParser(#[from] wasmparser::Error),
    #[error("Utf8 error {0}")]
    Utf8(#[from] core::str::Utf8Error),
    #[error("failed to convert number {0}")]
    IntConversion(#[from] core::num::TryFromIntError),
    #[error("failed to generate machine code from WASM {0}")]
    CodeGen(#[from] cranelift_codegen::CodegenError),
    #[error("failed to configure code generation {0}")]
    CodeGenSettings(#[from] cranelift_codegen::settings::SetError),
    #[error("failed to determine ISA {0}")]
    TargetLookupError(#[from] cranelift_codegen::isa::LookupError),
    #[error("expected {expected} values on top of translation stack, but found {found}")]
    EmptyStack { expected: usize, found: usize },
    #[error("expected frame on control stack")]
    EmptyControlStack,
    #[error("multiple start sections defined")]
    MultipleStartSections,
}

macro_rules! ensure {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err($err);
        }
    };
}

pub(crate) use ensure;
