//! Validation trait and error types for ECMA-119 wire-format structs.
//!
//! [`Validate`] is implemented on raw types in `raw.rs` and called by the
//! parser in strict mode.  The builder uses the character-set helpers
//! ([`is_d_char`], [`is_a_char`], [`is_file_id_char`]) directly.

use core::fmt;

/// Validates the semantic invariants of a wire-format struct.
///
/// Implementors should check every invariant required by ECMA-119 that cannot
/// be encoded as a Rust type (magic bytes, version constants, both-endian
/// agreement, reserved-field zeroes, etc.).  Implementations are *fail-fast*:
/// return the first error found.
pub trait Validate {
    fn validate(&self) -> Result<(), ValidationError>;
}

/// A validation failure on a single field.
#[derive(Debug)]
pub struct ValidationError {
    /// Dotted path to the field that failed,
    /// e.g. `"PrimaryVolumeDescriptor.logical_block_size"`.
    pub path: &'static str,
    pub kind: ValidationErrorKind,
}

impl ValidationError {
    /// Replace the path with a more specific one (used by parent validators to
    /// add context when propagating a child error).
    pub(crate) fn at(self, path: &'static str) -> Self {
        Self { path, ..self }
    }
}

#[derive(Debug)]
pub enum ValidationErrorKind {
    /// A magic byte sequence did not match the expected value.
    BadMagic { expected: &'static [u8] },
    /// The little-endian and big-endian copies of a both-endian field disagree.
    EndianMismatch { le: u64, be: u64 },
    /// A character in a string field is outside the allowed character set.
    BadCharacter { byte: u8, position: usize },
    /// A length field is outside its valid range.
    LengthOutOfRange { len: usize, min: usize, max: usize },
    /// A field did not have the expected value.
    BadValue {
        expected: &'static str,
        found: String,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.path)?;
        match &self.kind {
            ValidationErrorKind::BadMagic { expected } => {
                write!(f, "bad magic (expected {:?})", expected)
            }
            ValidationErrorKind::EndianMismatch { le, be } => {
                write!(f, "endian mismatch (LE={le:#x} BE={be:#x})")
            }
            ValidationErrorKind::LengthOutOfRange { len, min, max } => {
                write!(f, "length {len} out of range [{min}, {max}]")
            }
            ValidationErrorKind::BadValue { expected, found } => {
                write!(f, "expected value {expected}, found {found}")
            }
            ValidationErrorKind::BadCharacter { byte, position } => {
                write!(
                    f,
                    "invalid character {byte:04x} ({}) at offset {position}",
                    char::from(*byte)
                )
            }
        }
    }
}

impl core::error::Error for ValidationError {}

// ── Character-set helpers (ECMA-119 §7.4) ────────────────────────────────────

/// Returns `true` if `b` is a valid d-character (ECMA-119 §7.4.1).
///
/// d-characters: A–Z, 0–9, `_`
#[inline]
pub fn is_d_char(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'
}

/// Returns `true` if `b` is a valid a-character (ECMA-119 §7.4.2).
///
/// a-characters: d-characters plus SPACE and `! " % & ' ( ) * + , - . / : ; < = > ?`
#[inline]
pub fn is_a_char(b: u8) -> bool {
    b.is_ascii_uppercase()
        || b.is_ascii_lowercase()
        || b.is_ascii_digit()
        || matches!(
            b,
            b' ' | b'!'
                | b'"'
                | b'%'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b'-'
                | b'.'
                | b'/'
                | b':'
                | b';'
                | b'<'
                | b'='
                | b'>'
                | b'?'
        )
}

/// Returns `true` if `b` is valid inside a file identifier (ECMA-119 §7.5).
///
/// File identifiers use d-characters plus `.` (extension separator) and `;`
/// (version separator).
#[inline]
pub fn is_file_id_char(b: u8) -> bool {
    is_d_char(b) || matches!(b, b'.' | b';')
}
