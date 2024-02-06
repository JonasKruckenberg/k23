#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("UTF8")]
    Utf8(#[from] core::str::Utf8Error),
    #[error("invalid magic number")]
    InvalidMagicNumber,
    #[error("unexpected eof")]
    UnexpectedEof,
    #[error("unsupported external in element")]
    UnsupportedExternalInElement,
    #[error("unknown export description {0:#X}")]
    UnknownExportDescription(u8),
    #[error("unknown import description {0:#X}")]
    UnknownImportDescription(u8),
    #[error("unknown global mutability {0:#X}")]
    UnknownGlobalMutability(u8),
    #[error("unknown limit {0:#X}")]
    UnknownLimit(u8),
    #[error("unknown value type {0:#X}")]
    UnknownValType(u8),
    #[error("unknown reference type {0:#X}")]
    UnknownRefType(u8),
    #[error("unknown instruction {0:#04X}")]
    UnknownInstruction(u8),
    #[error("unimplemented instruction {0:#04X}")]
    UnimplementedInstruction(u8),
    #[error("unknown section {0}")]
    UnknownSection(u8),
    #[error("unknown function type")]
    UnknownFunctionType,
    #[error("invalid typed select arity {0}")]
    InvalidTypedSelectArity(u32),
    #[error("invalid function type")]
    InvalidFunctionType,
    #[error("string too long")]
    StringTooLong,
    #[error("modules has multiple start sections")]
    MultipleStartSections,
    #[error("MemArg alignment was too large")]
    AlignmentTooLarge,
}
