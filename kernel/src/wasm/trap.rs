use core::fmt;
use cranelift_codegen::ir::TrapCode;

const TRAP_OFFSET: u8 = 1;
pub const TRAP_INTERNAL_ASSERT: TrapCode =
    TrapCode::unwrap_user(Trap::InternalAssertionFailed as u8 + TRAP_OFFSET);
pub const TRAP_HEAP_MISALIGNED: TrapCode =
    TrapCode::unwrap_user(Trap::HeapMisaligned as u8 + TRAP_OFFSET);
pub const TRAP_TABLE_OUT_OF_BOUNDS: TrapCode =
    TrapCode::unwrap_user(Trap::TableOutOfBounds as u8 + TRAP_OFFSET);
pub const TRAP_INDIRECT_CALL_TO_NULL: TrapCode =
    TrapCode::unwrap_user(Trap::IndirectCallToNull as u8 + TRAP_OFFSET);
pub const TRAP_BAD_SIGNATURE: TrapCode =
    TrapCode::unwrap_user(Trap::BadSignature as u8 + TRAP_OFFSET);
pub const TRAP_UNREACHABLE: TrapCode =
    TrapCode::unwrap_user(Trap::UnreachableCodeReached as u8 + TRAP_OFFSET);
pub const TRAP_NULL_REFERENCE: TrapCode =
    TrapCode::unwrap_user(Trap::NullReference as u8 + TRAP_OFFSET);
pub const TRAP_I31_NULL_REFERENCE: TrapCode =
    TrapCode::unwrap_user(Trap::NullI31Ref as u8 + TRAP_OFFSET);

#[derive(Debug, Copy, Clone)]
pub enum Trap {
    /// Internal assertion failed
    InternalAssertionFailed,
    /// A wasm atomic operation was presented with a not-naturally-aligned linear-memory address.
    HeapMisaligned,
    /// Out-of-bounds access to a table.
    TableOutOfBounds,
    /// Indirect call to a null table entry.
    IndirectCallToNull,
    /// Signature mismatch on indirect call.
    BadSignature,
    /// Code that was supposed to have been unreachable was reached.
    UnreachableCodeReached,
    /// Call to a null reference.
    NullReference,
    /// Attempt to get the bits of a null `i31ref`.
    NullI31Ref,

    /// The current stack space was exhausted.
    StackOverflow,
    /// An out-of-bounds memory access.
    MemoryOutOfBounds,
    /// An integer arithmetic operation caused an overflow.
    IntegerOverflow,
    /// An integer division by zero.
    IntegerDivisionByZero,
    /// Failed float-to-int conversion.
    BadConversionToInteger,
}

impl fmt::Display for Trap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trap::InternalAssertionFailed => f.write_str("internal assertion failed"),
            Trap::HeapMisaligned => f.write_str("unaligned atomic operation"),
            Trap::TableOutOfBounds => f.write_str("out of bounds table access"),
            Trap::IndirectCallToNull => f.write_str("accessed uninitialized table element"),
            Trap::BadSignature => f.write_str("indirect call signature mismatch"),
            Trap::UnreachableCodeReached => f.write_str("unreachable code executed"),
            Trap::NullReference => f.write_str("null reference called"),
            Trap::NullI31Ref => f.write_str("null i32 reference called"),

            Trap::StackOverflow => f.write_str("call stack exhausted"),
            Trap::MemoryOutOfBounds => f.write_str("out of bounds memory access"),
            Trap::IntegerOverflow => f.write_str("integer overflow"),
            Trap::IntegerDivisionByZero => f.write_str("integer divide by zero"),
            Trap::BadConversionToInteger => f.write_str("invalid conversion to integer"),
        }
    }
}

impl core::error::Error for Trap {}

impl Trap {
    pub(crate) fn from_trap_code(code: TrapCode) -> Option<Self> {
        match code {
            TrapCode::STACK_OVERFLOW => Some(Trap::StackOverflow),
            TrapCode::HEAP_OUT_OF_BOUNDS => Some(Trap::MemoryOutOfBounds),
            TrapCode::INTEGER_OVERFLOW => Some(Trap::IntegerOverflow),
            TrapCode::INTEGER_DIVISION_BY_ZERO => Some(Trap::IntegerDivisionByZero),
            TrapCode::BAD_CONVERSION_TO_INTEGER => Some(Trap::BadConversionToInteger),

            TRAP_INTERNAL_ASSERT => Some(Trap::InternalAssertionFailed),
            TRAP_HEAP_MISALIGNED => Some(Trap::HeapMisaligned),
            TRAP_TABLE_OUT_OF_BOUNDS => Some(Trap::TableOutOfBounds),
            TRAP_INDIRECT_CALL_TO_NULL => Some(Trap::IndirectCallToNull),
            TRAP_BAD_SIGNATURE => Some(Trap::BadSignature),
            TRAP_UNREACHABLE => Some(Trap::UnreachableCodeReached),
            TRAP_NULL_REFERENCE => Some(Trap::NullReference),
            TRAP_I31_NULL_REFERENCE => Some(Trap::NullI31Ref),
            c => {
                log::warn!("unknown trap code {c}");
                None
            }
        }
    }
}

impl From<Trap> for u8 {
    fn from(value: Trap) -> Self {
        match value {
            Trap::InternalAssertionFailed => 0,
            Trap::HeapMisaligned => 1,
            Trap::TableOutOfBounds => 2,
            Trap::IndirectCallToNull => 3,
            Trap::BadSignature => 4,
            Trap::UnreachableCodeReached => 5,
            Trap::NullReference => 6,
            Trap::NullI31Ref => 7,

            Trap::StackOverflow => 8,
            Trap::MemoryOutOfBounds => 9,
            Trap::IntegerOverflow => 10,
            Trap::IntegerDivisionByZero => 11,
            Trap::BadConversionToInteger => 12,
        }
    }
}

impl TryFrom<u8> for Trap {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::InternalAssertionFailed),
            1 => Ok(Self::HeapMisaligned),
            2 => Ok(Self::TableOutOfBounds),
            3 => Ok(Self::IndirectCallToNull),
            4 => Ok(Self::BadSignature),
            5 => Ok(Self::UnreachableCodeReached),
            6 => Ok(Self::NullReference),
            7 => Ok(Self::NullI31Ref),

            8 => Ok(Self::StackOverflow),
            9 => Ok(Self::MemoryOutOfBounds),
            10 => Ok(Self::IntegerOverflow),
            11 => Ok(Self::IntegerDivisionByZero),
            12 => Ok(Self::BadConversionToInteger),
            _ => Err(()),
        }
    }
}
