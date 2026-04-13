//! Builders for creating the on-disk directory hierarchy

use std::path::Path;
use std::{fs, io};

use super::BuildError;
use crate::validate::{ValidationError, ValidationErrorKind, is_d_char, is_file_id_char};

#[derive(Debug)]
pub enum FileSource<'a> {
    /// Data already in memory.
    InMemory(&'a [u8]),
    /// Read from an external source at build time.
    OnDisk { len: u64, reader: fs::File },
}

impl<'a> FileSource<'a> {
    pub fn len(&self) -> usize {
        match self {
            FileSource::InMemory(items) => items.len(),
            FileSource::OnDisk { len, .. } => *len as usize,
        }
    }

    pub fn from_bytes(bytes: &'a [u8]) -> Self {
        Self::InMemory(bytes)
    }

    pub fn from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        let len = file.metadata()?.len();

        Ok(Self::OnDisk { len, reader: file })
    }
}

#[derive(Debug, Default)]
pub struct DirectoryBuilder<'a> {
    pub(super) name: &'a str,
    pub(super) subdirs: Vec<DirectoryBuilder<'a>>,
    pub(super) files: Vec<(&'a str, FileSource<'a>)>,
}

impl<'a> DirectoryBuilder<'a> {
    pub fn add_file(
        &mut self,
        name: &'a str,
        source: FileSource<'a>,
    ) -> Result<&mut Self, BuildError> {
        validate_file_identifier(name).map_err(BuildError::Invalid)?;
        self.files.push((name, source));
        Ok(self)
    }

    pub fn add_dir(&mut self, name: &'a str) -> Result<&mut DirectoryBuilder<'a>, BuildError> {
        validate_dir_identifier(name).map_err(BuildError::Invalid)?;
        self.subdirs.push(DirectoryBuilder {
            name,
            subdirs: Vec::new(),
            files: Vec::new(),
        });

        Ok(self.subdirs.last_mut().unwrap())
    }
}

/// Validates a file identifier (name + optional `.EXT;VER`).
///
/// Accepts d-characters, `.`, and `;`.  The identifier must be non-empty and
/// at most 30 bytes (leaving room for the `;1` version suffix on Level 1
/// images).
fn validate_file_identifier(name: &str) -> Result<(), ValidationError> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 30 {
        return Err(ValidationError {
            path: "FileIdentifier",
            kind: ValidationErrorKind::LengthOutOfRange {
                len: bytes.len(),
                min: 1,
                max: 30,
            },
        });
    }
    for (i, &b) in bytes.iter().enumerate() {
        if !is_file_id_char(b) {
            return Err(ValidationError {
                path: "FileIdentifier",
                kind: ValidationErrorKind::BadCharacter {
                    byte: b,
                    position: i,
                },
            });
        }
    }
    Ok(())
}

/// Validates a directory identifier.
///
/// Directory identifiers use d-characters only (no `.` or `;`), and are at
/// most 31 bytes.
fn validate_dir_identifier(name: &str) -> Result<(), ValidationError> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 31 {
        return Err(ValidationError {
            path: "DirectoryIdentifier",
            kind: ValidationErrorKind::LengthOutOfRange {
                len: bytes.len(),
                min: 1,
                max: 31,
            },
        });
    }
    for (i, &b) in bytes.iter().enumerate() {
        if !is_d_char(b) && b != b'.' && b != b';' {
            return Err(ValidationError {
                path: "DirectoryIdentifier",
                kind: ValidationErrorKind::BadCharacter {
                    byte: b,
                    position: i,
                },
            });
        }
    }
    Ok(())
}
