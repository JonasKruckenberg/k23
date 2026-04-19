//! Validation trait and character-set helpers for ECMA-119 wire-format structs.
//!
//! [`Validate`] is implemented on raw types in `raw.rs` and called by the
//! parser in strict mode.  The builder uses the character-set helpers
//! ([`is_d_char`], [`is_a_char`], [`is_file_id_char`]) directly.

/// Validates the semantic invariants of a wire-format struct.
///
/// Implementors should check every invariant required by ECMA-119 that cannot
/// be encoded as a Rust type (magic bytes, version constants, both-endian
/// agreement, reserved-field zeroes, etc.).  Implementations are *fail-fast*:
/// return the first error found.
pub trait Validate {
    fn validate(&self) -> anyhow::Result<()>;
}

// ── Character-set helpers (ECMA-119 §7.4) ────────────────────────────────────

/// Returns `true` if `b` is a valid d-character (ECMA-119 §7.4.1).
///
/// d-characters: A–Z, 0–9, `_`
#[inline]
pub(crate) fn is_d_char(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'
}

/// Returns `true` if `b` is a valid a-character (ECMA-119 §7.4.2).
///
/// a-characters: d-characters plus SPACE and `! " % & ' ( ) * + , - . / : ; < = > ?`
#[inline]
pub(crate) fn is_a_char(b: u8) -> bool {
    b.is_ascii_uppercase()
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
pub(crate) fn is_file_id_char(b: u8) -> bool {
    is_d_char(b) || matches!(b, b'.' | b';')
}

/// Validates a file identifier (name + optional `.EXT;VER`).
///
/// Accepts d-characters, `.`, and `;`.  The identifier must be non-empty and
/// at most 30 bytes (leaving room for the `;1` version suffix on Level 1
/// images).
pub(crate) fn validate_file_identifier(bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= 30,
        "file identifier length {} out of range [1, 30]",
        bytes.len()
    );
    for (i, &b) in bytes.iter().enumerate() {
        anyhow::ensure!(
            is_file_id_char(b),
            "file identifier: invalid character {:#04x} ({}) at position {}",
            b,
            char::from(b),
            i,
        );
    }
    Ok(())
}

/// Validates a directory identifier.
///
/// Directory identifiers use d-characters only (no `.` or `;`), and are at
/// most 31 bytes.
pub(crate) fn validate_dir_identifier(bytes: &[u8]) -> anyhow::Result<()> {
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= 31,
        "directory identifier length {} out of range [1, 31]",
        bytes.len()
    );
    for (i, &b) in bytes.iter().enumerate() {
        anyhow::ensure!(
            is_d_char(b),
            "directory identifier: invalid character {:#04x} ({}) at position {}",
            b,
            char::from(b),
            i,
        );
    }
    Ok(())
}
