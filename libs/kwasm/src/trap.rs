use core::fmt;

#[derive(Debug, Copy, Clone)]
pub enum TrapKind {
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

    /// Used to indicate that a trap was raised by atomic wait operations on non shared memory.
    AtomicWaitNonSharedMemory,
}

impl fmt::Display for TrapKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrapKind::InternalAssertionFailed => f.write_str("internal assertion failed"),
            TrapKind::HeapMisaligned => f.write_str("unaligned atomic operation"),
            TrapKind::TableOutOfBounds => f.write_str("out of bounds table access"),
            TrapKind::IndirectCallToNull => f.write_str("accessed uninitialized table element"),
            TrapKind::BadSignature => f.write_str("indirect call signature mismatch"),
            TrapKind::UnreachableCodeReached => f.write_str("unreachable code executed"),
            TrapKind::NullReference => f.write_str("null reference called"),
            TrapKind::NullI31Ref => f.write_str("null i32 reference called"),

            TrapKind::StackOverflow => f.write_str("call stack exhausted"),
            TrapKind::MemoryOutOfBounds => f.write_str("out of bounds memory access"),
            TrapKind::IntegerOverflow => f.write_str("integer overflow"),
            TrapKind::IntegerDivisionByZero => f.write_str("integer divide by zero"),
            TrapKind::BadConversionToInteger => f.write_str("invalid conversion to integer"),

            TrapKind::AtomicWaitNonSharedMemory => f.write_str("atomic wait on non-shared memory"),
        }
    }
}

impl core::error::Error for TrapKind {}
