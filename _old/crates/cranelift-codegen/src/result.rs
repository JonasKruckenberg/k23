//! Result and error types representing the outcome of compiling a function.

use regalloc2::checker::CheckerErrors;

use crate::ir::pcc::PccError;
use crate::{ir::Function, verifier::VerifierErrors};
use alloc::string::String;

/// A compilation error.
///
/// When Cranelift fails to compile a function, it will return one of these error codes.
#[derive(Debug, onlyerror::Error)]
pub enum CodegenError {
    /// A list of IR verifier errors.
    ///
    /// This always represents a bug, either in the code that generated IR for Cranelift, or a bug
    /// in Cranelift itself.
    #[error("Verifier errors")]
    Verifier(#[from] VerifierErrors),

    /// An implementation limit was exceeded.
    ///
    /// Cranelift can compile very large and complicated functions, but the [implementation has
    /// limits][limits] that cause compilation to fail when they are exceeded.
    ///
    /// [limits]: https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/docs/ir.md#implementation-limits
    #[error("Implementation limit exceeded")]
    ImplLimitExceeded,

    /// The code size for the function is too large.
    ///
    /// Different target ISAs may impose a limit on the size of a compiled function. If that limit
    /// is exceeded, compilation fails.
    #[error("Code for function is too large")]
    CodeTooLarge,

    /// Something is not supported by the code generator. This might be an indication that a
    /// feature is used without explicitly enabling it, or that something is temporarily
    /// unsupported by a given target backend.
    #[error("Unsupported feature: {0}")]
    Unsupported(String),

    /// A failure to map Cranelift register representation to a DWARF register representation.
    #[cfg(feature = "unwind")]
    #[error("Register mapping error")]
    RegisterMappingError(#[from] crate::isa::unwind::systemv::RegisterMappingError),

    /// Register allocator internal error discovered by the symbolic checker.
    #[error("Regalloc validation errors: {0:?}")]
    Regalloc(CheckerErrors),

    /// Proof-carrying-code validation error.
    #[error("Proof-carrying-code validation error: {0}")]
    Pcc(#[from] PccError),
}

/// A convenient alias for a `Result` that uses `CodegenError` as the error type.
pub type CodegenResult<T> = Result<T, CodegenError>;

/// Compilation error, with the accompanying function to help printing it.
pub struct CompileError<'a> {
    /// Underlying `CodegenError` that triggered the error.
    pub inner: CodegenError,
    /// Function we tried to compile, for display purposes.
    pub func: &'a Function,
}

// By default, have `CompileError` be displayed as the internal error, and let consumers care if
// they want to use the func field for adding details.
impl<'a> core::fmt::Debug for CompileError<'a> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.inner.fmt(f)
    }
}

/// A convenient alias for a `Result` that uses `CompileError` as the error type.
pub type CompileResult<'a, T> = Result<T, CompileError<'a>>;
