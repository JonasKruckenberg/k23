// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Error types for ISO 9660 image creation.

use core::fmt;

/// Errors that can occur during ISO 9660 image creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file identifier exceeds the maximum length (222 bytes for ISO level 1,
    /// 207 bytes for directory identifiers).
    IdentifierTooLong {
        /// The identifier that was too long
        identifier: &'static str,
        /// The maximum allowed length
        max_length: usize,
    },
    /// The file identifier contains invalid characters.
    InvalidIdentifier(&'static str),
    /// The directory nesting exceeds the maximum depth (8 levels for ISO level 1).
    DirectoryTooDeep,
    /// The path table exceeds the maximum size.
    PathTableTooLarge,
    /// The volume space exceeds the maximum size (2^32 - 1 sectors).
    VolumeSpaceTooLarge,
    /// No boot image was provided for a bootable ISO.
    NoBootImage,
    /// The boot image is too large.
    BootImageTooLarge,
    /// A required field was not set.
    MissingField(&'static str),
    /// The buffer is too small for the operation.
    BufferTooSmall {
        /// The required size
        required: usize,
        /// The actual size
        actual: usize,
    },
    /// An I/O error occurred during writing.
    WriteError,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::IdentifierTooLong {
                identifier,
                max_length,
            } => {
                write!(
                    f,
                    "identifier '{identifier}' exceeds maximum length of {max_length} bytes"
                )
            }
            Error::InvalidIdentifier(id) => {
                write!(f, "invalid identifier: {id}")
            }
            Error::DirectoryTooDeep => {
                write!(f, "directory nesting exceeds maximum depth")
            }
            Error::PathTableTooLarge => {
                write!(f, "path table exceeds maximum size")
            }
            Error::VolumeSpaceTooLarge => {
                write!(f, "volume space exceeds maximum size")
            }
            Error::NoBootImage => {
                write!(f, "no boot image provided for bootable ISO")
            }
            Error::BootImageTooLarge => {
                write!(f, "boot image exceeds maximum size")
            }
            Error::MissingField(field) => {
                write!(f, "missing required field: {field}")
            }
            Error::BufferTooSmall { required, actual } => {
                write!(
                    f,
                    "buffer too small: required {required} bytes, got {actual}"
                )
            }
            Error::WriteError => {
                write!(f, "write error")
            }
        }
    }
}

/// Result type for ISO 9660 operations.
pub type Result<T> = core::result::Result<T, Error>;
