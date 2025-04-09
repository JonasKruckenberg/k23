// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use cranelift_codegen::ir::TrapCode;

const TRAP_OFFSET: u8 = 1;
pub const TRAP_INTERNAL_ASSERT: TrapCode =
    TrapCode::unwrap_user(TrapKind::InternalAssertionFailed as u8 + TRAP_OFFSET);
pub const TRAP_HEAP_MISALIGNED: TrapCode =
    TrapCode::unwrap_user(TrapKind::HeapMisaligned as u8 + TRAP_OFFSET);
pub const TRAP_TABLE_OUT_OF_BOUNDS: TrapCode =
    TrapCode::unwrap_user(TrapKind::TableOutOfBounds as u8 + TRAP_OFFSET);
pub const TRAP_INDIRECT_CALL_TO_NULL: TrapCode =
    TrapCode::unwrap_user(TrapKind::IndirectCallToNull as u8 + TRAP_OFFSET);
pub const TRAP_BAD_SIGNATURE: TrapCode =
    TrapCode::unwrap_user(TrapKind::BadSignature as u8 + TRAP_OFFSET);
pub const TRAP_UNREACHABLE: TrapCode =
    TrapCode::unwrap_user(TrapKind::UnreachableCodeReached as u8 + TRAP_OFFSET);
pub const TRAP_NULL_REFERENCE: TrapCode =
    TrapCode::unwrap_user(TrapKind::NullReference as u8 + TRAP_OFFSET);
pub const TRAP_I31_NULL_REFERENCE: TrapCode =
    TrapCode::unwrap_user(TrapKind::NullI31Ref as u8 + TRAP_OFFSET);
pub const TRAP_ATOMIC_WAIT_NON_SHARED_MEMORY: TrapCode =
    TrapCode::unwrap_user(TrapKind::AtomicWaitNonSharedMemory as u8 + TRAP_OFFSET);

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

impl TrapKind {
    pub(crate) fn from_trap_code(code: TrapCode) -> Option<Self> {
        match code {
            TrapCode::STACK_OVERFLOW => Some(TrapKind::StackOverflow),
            TrapCode::HEAP_OUT_OF_BOUNDS => Some(TrapKind::MemoryOutOfBounds),
            TrapCode::INTEGER_OVERFLOW => Some(TrapKind::IntegerOverflow),
            TrapCode::INTEGER_DIVISION_BY_ZERO => Some(TrapKind::IntegerDivisionByZero),
            TrapCode::BAD_CONVERSION_TO_INTEGER => Some(TrapKind::BadConversionToInteger),

            TRAP_INTERNAL_ASSERT => Some(TrapKind::InternalAssertionFailed),
            TRAP_HEAP_MISALIGNED => Some(TrapKind::HeapMisaligned),
            TRAP_TABLE_OUT_OF_BOUNDS => Some(TrapKind::TableOutOfBounds),
            TRAP_INDIRECT_CALL_TO_NULL => Some(TrapKind::IndirectCallToNull),
            TRAP_BAD_SIGNATURE => Some(TrapKind::BadSignature),
            TRAP_UNREACHABLE => Some(TrapKind::UnreachableCodeReached),
            TRAP_NULL_REFERENCE => Some(TrapKind::NullReference),
            TRAP_I31_NULL_REFERENCE => Some(TrapKind::NullI31Ref),
            
            TRAP_ATOMIC_WAIT_NON_SHARED_MEMORY => Some(TrapKind::AtomicWaitNonSharedMemory),
            
            c => {
                tracing::warn!("unknown trap code {c}");
                None
            }
        }
    }
}

impl From<TrapKind> for u8 {
    fn from(value: TrapKind) -> Self {
        match value {
            TrapKind::InternalAssertionFailed => 0,
            TrapKind::HeapMisaligned => 1,
            TrapKind::TableOutOfBounds => 2,
            TrapKind::IndirectCallToNull => 3,
            TrapKind::BadSignature => 4,
            TrapKind::UnreachableCodeReached => 5,
            TrapKind::NullReference => 6,
            TrapKind::NullI31Ref => 7,

            TrapKind::StackOverflow => 8,
            TrapKind::MemoryOutOfBounds => 9,
            TrapKind::IntegerOverflow => 10,
            TrapKind::IntegerDivisionByZero => 11,
            TrapKind::BadConversionToInteger => 12,
            
            TrapKind::AtomicWaitNonSharedMemory => 13,
        }
    }
}

impl TryFrom<u8> for TrapKind {
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
            
            13 => Ok(Self::AtomicWaitNonSharedMemory),
            
            _ => Err(()),
        }
    }
}
