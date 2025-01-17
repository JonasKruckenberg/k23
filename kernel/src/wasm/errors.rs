use crate::wasm::backtrace::RawWasmBacktrace;
use crate::wasm::translate::EntityType;
use crate::wasm::trap::Trap;
use alloc::format;
use alloc::string::{String, ToString};
use core::fmt;
use cranelift_codegen::CodegenError;

/// Convenience macro for creating an `Error::Unsupported` variant.
#[macro_export]
macro_rules! wasm_unsupported {
    ($($arg:tt)*) => { $crate::wasm::Error::Unsupported(::alloc::format!($($arg)*)) }
}

/// Error type for the crate
#[derive(Debug)]
pub enum Error {
    /// The input WebAssembly code is invalid.
    InvalidWebAssembly {
        /// A string describing the validation error.
        message: String,
        /// The bytecode offset where the error occurred.
        offset: usize,
    },
    /// A required import was not provided.
    MissingImport {
        /// The module name of the import.
        module: String,
        /// The field name of the import.
        field: String,
        /// The type of the import.
        type_: EntityType,
    },
    /// The WebAssembly code used an unsupported feature.
    Unsupported(String),
    /// Failed to compile a function.
    Cranelift {
        /// The name of the function that failed to compile.
        func_name: String,
        /// A human-readable description of the error.
        message: String,
    },
    /// Failed to parse DWARF debug information.
    Gimli(gimli::Error),
    // /// Failed to parse a wat file.
    // Wat(wat::Error),
    /// A WebAssembly trap occurred.
    Trap {
        /// The program counter where this trap originated.
        pc: usize,
        /// The address of the inaccessible data or zero if trap wasn't caused by data access.
        faulting_addr: usize,
        /// The trap that occurred.
        trap: Trap,
        /// A human-readable description of the trap.
        message: String,
        backtrace: RawWasmBacktrace,
    },
    /// Memory mapping failed
    MmapFailed,
    /// The name is already defined.
    AlreadyDefined {
        /// The defined module name.
        module: String,
        /// The defined field name.
        field: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWebAssembly { message, offset } => {
                f.write_fmt(format_args!("invalid WASM input at {offset}: {message}"))
            }
            Self::MissingImport {
                module,
                field,
                type_,
            } => {
                let type_ = match type_ {
                    EntityType::Function(_) => "function",
                    EntityType::Table(_) => "table",
                    EntityType::Memory(_) => "memory",
                    EntityType::Global(_) => "global",
                };
                f.write_fmt(format_args!(
                    "Missing required import {module}::{field} ({type_})"
                ))
            }
            Self::Unsupported(feature) => f.write_fmt(format_args!(
                "Feature used by the WebAssembly code is not supported: {feature}"
            )),
            Self::Cranelift { func_name, message } => f.write_fmt(format_args!(
                "failed to compile function {func_name}: {message}"
            )),
            Self::Gimli(e) => {
                f.write_fmt(format_args!("Failed to parse DWARF debug information: {e}"))
            }
            // Self::Wat(e) => f.write_fmt(format_args!("Failed to parse wat: {e}")),
            Self::Trap { trap, message, .. } => {
                f.write_fmt(format_args!("{message}. Reason {trap}"))?;
                Ok(())
            }
            Self::MmapFailed => f.write_str("Memory mapping failed"),
            Self::AlreadyDefined { module, field } => {
                f.write_fmt(format_args!("Name {module}::{field} is already defined"))
            }
        }
    }
}

impl From<wasmparser::BinaryReaderError> for Error {
    fn from(e: wasmparser::BinaryReaderError) -> Self {
        Self::InvalidWebAssembly {
            message: e.message().into(),
            offset: e.offset(),
        }
    }
}

impl From<cranelift_codegen::CompileError<'_>> for Error {
    fn from(e: cranelift_codegen::CompileError<'_>) -> Self {
        Self::Cranelift {
            func_name: e.func.name.to_string(),
            message: match e.inner {
                CodegenError::Verifier(errs) => format!("Verifier errors {errs}"),
                CodegenError::ImplLimitExceeded => "Implementation limit exceeded".to_string(),
                CodegenError::CodeTooLarge => "Code for function is too large".to_string(),
                CodegenError::Unsupported(feature) => format!("Unsupported feature: {feature}"),
                CodegenError::Regalloc(errors) => format!("Regalloc validation errors: {errors:?}"),
                CodegenError::Pcc(e) => format!("Proof-carrying-code validation error: {e:?}"),
                // CodegenError::RegisterMappingError(e) => format!("Register mapping error {e}"),
            },
        }
    }
}

impl From<gimli::Error> for Error {
    fn from(value: gimli::Error) -> Self {
        Self::Gimli(value)
    }
}

// impl From<wat::Error> for Error {
//     fn from(value: wat::Error) -> Self {
//         Self::Wat(value)
//     }
// }

impl core::error::Error for Error {}

#[derive(Copy, Clone, Debug)]
pub struct SizeOverflow;

impl fmt::Display for SizeOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("size overflow calculating memory size")
    }
}

impl core::error::Error for SizeOverflow {}
