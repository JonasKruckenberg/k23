// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Directory Record structures for ISO 9660 filesystems.
//!
//! Directory records describe files and directories within the filesystem.
//! Each record has a variable length (minimum 34 bytes) and contains
//! metadata about the file/directory including location, size, and attributes.
//!
//! Reference: ECMA-119 Section 9.1

use crate::types::{BothEndian16, BothEndian32, DirectoryDateTime};

/// File flags for directory records.
///
/// Reference: ECMA-119 Section 9.1.6
pub mod flags {
    /// File is hidden (existence bit).
    pub const HIDDEN: u8 = 0x01;
    /// Entry is a directory.
    pub const DIRECTORY: u8 = 0x02;
    /// Entry is an associated file.
    pub const ASSOCIATED: u8 = 0x04;
    /// Extended attribute record contains information about the format of the record.
    pub const RECORD: u8 = 0x08;
    /// Owner and group permissions are specified in the extended attribute record.
    pub const PROTECTION: u8 = 0x10;
    /// Reserved bits 5-6.
    pub const RESERVED_5: u8 = 0x20;
    pub const RESERVED_6: u8 = 0x40;
    /// This is not the final directory record for this file (multi-extent).
    pub const MULTI_EXTENT: u8 = 0x80;
}

/// Maximum length of a file identifier in ISO 9660 Level 1.
pub const MAX_FILE_IDENTIFIER_LENGTH: usize = 30;

/// Maximum length of a directory identifier in ISO 9660 Level 1.
pub const MAX_DIRECTORY_IDENTIFIER_LENGTH: usize = 31;

/// Directory Record structure.
///
/// This structure has a fixed 33-byte header followed by a variable-length
/// file identifier and optional padding.
///
/// Reference: ECMA-119 Section 9.1
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct DirectoryRecord {
    /// Length of this directory record
    pub length: u8,
    /// Extended Attribute Record length
    pub extended_attribute_length: u8,
    /// Location of extent (first sector of file data)
    pub location: BothEndian32,
    /// Data length (file size in bytes)
    pub data_length: BothEndian32,
    /// Recording date and time
    pub recording_date_time: DirectoryDateTime,
    /// File flags
    pub file_flags: u8,
    /// File unit size (for interleaved files, 0 otherwise)
    pub file_unit_size: u8,
    /// Interleave gap size (for interleaved files, 0 otherwise)
    pub interleave_gap_size: u8,
    /// Volume sequence number where this extent is recorded
    pub volume_sequence_number: BothEndian16,
    /// Length of file identifier
    pub file_identifier_length: u8,
    /// File identifier (variable length, followed by padding if even length).
    /// For the root directory entry in the PVD, this is a single byte (0x00).
    pub file_identifier: [u8; 1],
}

/// Minimum size of a directory record (header only, with 1-byte identifier).
pub const DIRECTORY_RECORD_MIN_SIZE: usize = 34;

impl DirectoryRecord {
    /// Creates a new root directory record.
    ///
    /// The root directory record uses a single 0x00 byte as the identifier.
    #[must_use]
    pub fn new_root() -> Self {
        Self {
            length: 34,
            extended_attribute_length: 0,
            location: BothEndian32::new(0),
            data_length: BothEndian32::new(0),
            recording_date_time: DirectoryDateTime::unspecified(),
            file_flags: flags::DIRECTORY,
            file_unit_size: 0,
            interleave_gap_size: 0,
            volume_sequence_number: BothEndian16::new(1),
            file_identifier_length: 1,
            file_identifier: [0x00],
        }
    }

    /// Returns the actual size of this directory record.
    #[must_use]
    pub fn record_length(&self) -> usize {
        self.length as usize
    }

    /// Returns whether this record represents a directory.
    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.file_flags & flags::DIRECTORY != 0
    }
}

const _: () = assert!(size_of::<DirectoryRecord>() == DIRECTORY_RECORD_MIN_SIZE);

/// Builder for creating directory records.
///
/// This builder handles the variable-length nature of directory records,
/// including proper padding.
pub struct DirectoryRecordBuilder {
    /// Sector location of the file/directory data
    location: u32,
    /// Size of the file/directory data in bytes
    data_length: u32,
    /// File flags
    file_flags: u8,
    /// Recording date/time
    recording_date_time: DirectoryDateTime,
    /// File identifier (name)
    identifier: [u8; MAX_FILE_IDENTIFIER_LENGTH + 2],
    /// Length of the identifier
    identifier_length: u8,
}

impl DirectoryRecordBuilder {
    /// Creates a new builder for a file.
    #[must_use]
    pub fn new_file(name: &str, location: u32, size: u32) -> Self {
        let mut builder = Self {
            location,
            data_length: size,
            file_flags: 0,
            recording_date_time: DirectoryDateTime::unspecified(),
            identifier: [0; MAX_FILE_IDENTIFIER_LENGTH + 2],
            identifier_length: 0,
        };
        builder.set_identifier(name);
        builder
    }

    /// Creates a new builder for a directory.
    #[must_use]
    pub fn new_directory(name: &str, location: u32, size: u32) -> Self {
        let mut builder = Self {
            location,
            data_length: size,
            file_flags: flags::DIRECTORY,
            recording_date_time: DirectoryDateTime::unspecified(),
            identifier: [0; MAX_FILE_IDENTIFIER_LENGTH + 2],
            identifier_length: 0,
        };
        builder.set_identifier(name);
        builder
    }

    /// Creates a "." (self) directory entry.
    #[must_use]
    pub fn new_self_entry(location: u32, size: u32) -> Self {
        Self {
            location,
            data_length: size,
            file_flags: flags::DIRECTORY,
            recording_date_time: DirectoryDateTime::unspecified(),
            identifier: [0; MAX_FILE_IDENTIFIER_LENGTH + 2],
            identifier_length: 1,
        }
    }

    /// Creates a ".." (parent) directory entry.
    #[must_use]
    pub fn new_parent_entry(location: u32, size: u32) -> Self {
        let mut identifier = [0; MAX_FILE_IDENTIFIER_LENGTH + 2];
        identifier[0] = 0x01;
        Self {
            location,
            data_length: size,
            file_flags: flags::DIRECTORY,
            recording_date_time: DirectoryDateTime::unspecified(),
            identifier,
            identifier_length: 1,
        }
    }

    /// Sets the file identifier (name).
    fn set_identifier(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let len = bytes.len().min(MAX_FILE_IDENTIFIER_LENGTH);
        self.identifier[..len].copy_from_slice(&bytes[..len]);
        self.identifier_length = len as u8;
    }

    /// Sets the recording date/time.
    pub fn set_date_time(&mut self, date_time: DirectoryDateTime) {
        self.recording_date_time = date_time;
    }

    /// Sets the hidden flag.
    pub fn set_hidden(&mut self, hidden: bool) {
        if hidden {
            self.file_flags |= flags::HIDDEN;
        } else {
            self.file_flags &= !flags::HIDDEN;
        }
    }

    /// Calculates the total size of the directory record.
    #[must_use]
    pub fn record_size(&self) -> usize {
        // Base size (33 bytes) + identifier length + padding (if even length)
        let base = 33 + self.identifier_length as usize;
        if base % 2 == 0 {
            base
        } else {
            base + 1
        }
    }

    /// Writes the directory record to a buffer.
    ///
    /// Returns the number of bytes written.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is too small.
    pub fn write_to(&self, buf: &mut [u8]) -> usize {
        let record_size = self.record_size();
        assert!(buf.len() >= record_size, "buffer too small for directory record");

        // Zero the buffer first
        buf[..record_size].fill(0);

        // Length of directory record
        buf[0] = record_size as u8;
        // Extended attribute record length
        buf[1] = 0;
        // Location of extent (both-endian: 4 bytes LE + 4 bytes BE)
        let location = BothEndian32::new(self.location);
        buf[2..6].copy_from_slice(&location.le_bytes()[..]);
        buf[6..10].copy_from_slice(&location.be_bytes()[..]);
        // Data length (both-endian: 4 bytes LE + 4 bytes BE)
        let data_length = BothEndian32::new(self.data_length);
        buf[10..14].copy_from_slice(&data_length.le_bytes()[..]);
        buf[14..18].copy_from_slice(&data_length.be_bytes()[..]);
        // Recording date and time
        buf[18] = self.recording_date_time.years_since_1900;
        buf[19] = self.recording_date_time.month;
        buf[20] = self.recording_date_time.day;
        buf[21] = self.recording_date_time.hour;
        buf[22] = self.recording_date_time.minute;
        buf[23] = self.recording_date_time.second;
        buf[24] = self.recording_date_time.gmt_offset as u8;
        // File flags
        buf[25] = self.file_flags;
        // File unit size
        buf[26] = 0;
        // Interleave gap size
        buf[27] = 0;
        // Volume sequence number (both-endian: 2 bytes LE + 2 bytes BE)
        let vsn = BothEndian16::new(1);
        buf[28..30].copy_from_slice(vsn.le_bytes());
        buf[30..32].copy_from_slice(vsn.be_bytes());
        // File identifier length
        buf[32] = self.identifier_length;
        // File identifier
        let id_len = self.identifier_length as usize;
        buf[33..33 + id_len].copy_from_slice(&self.identifier[..id_len]);

        record_size
    }
}

/// Converts a filename to ISO 9660 Level 1 format.
///
/// ISO 9660 Level 1 filenames:
/// - Maximum 8 characters for the name
/// - Maximum 3 characters for the extension
/// - Separated by a dot
/// - Only uppercase A-Z, 0-9, and underscore
/// - Files must end with ";1" version number
///
/// # Arguments
///
/// * `name` - The input filename
/// * `is_directory` - Whether this is a directory name
///
/// # Returns
///
/// The converted ISO 9660 filename.
#[must_use]
pub fn to_iso9660_name(name: &str, is_directory: bool) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut pos = 0;

    // Convert to uppercase and filter invalid characters
    let name_upper = name.to_ascii_uppercase();
    let (base, ext) = if let Some(dot_pos) = name_upper.rfind('.') {
        (&name_upper[..dot_pos], Some(&name_upper[dot_pos + 1..]))
    } else {
        (name_upper.as_str(), None)
    };

    // Write base name (max 8 chars for files, 31 for directories)
    let max_base = if is_directory { 31 } else { 8 };
    for ch in base.chars().take(max_base) {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            result[pos] = ch as u8;
            pos += 1;
        }
    }

    if !is_directory {
        // Add dot and extension for files
        result[pos] = b'.';
        pos += 1;

        if let Some(ext) = ext {
            for ch in ext.chars().take(3) {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    result[pos] = ch as u8;
                    pos += 1;
                }
            }
        }

        // Add version number
        result[pos] = b';';
        pos += 1;
        result[pos] = b'1';
    }

    result
}

/// Calculates the ISO 9660 filename length.
#[must_use]
pub fn iso9660_name_length(name: &[u8; 32]) -> u8 {
    name.iter()
        .position(|&b| b == 0)
        .unwrap_or(32) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directory_record_size() {
        assert_eq!(size_of::<DirectoryRecord>(), DIRECTORY_RECORD_MIN_SIZE);
    }

    #[test]
    fn test_root_record() {
        let root = DirectoryRecord::new_root();
        assert_eq!(root.length, 34);
        assert!(root.is_directory());
        assert_eq!(root.file_identifier_length, 1);
        assert_eq!(root.file_identifier[0], 0x00);
    }

    #[test]
    fn test_directory_record_builder() {
        let builder = DirectoryRecordBuilder::new_file("TEST.TXT", 100, 512);
        let size = builder.record_size();
        assert!(size >= DIRECTORY_RECORD_MIN_SIZE);

        let mut buf = [0u8; 128];
        let written = builder.write_to(&mut buf);
        assert_eq!(written, size);
        assert_eq!(buf[0], size as u8);
    }

    #[test]
    fn test_self_entry() {
        let builder = DirectoryRecordBuilder::new_self_entry(100, 2048);
        let _size = builder.record_size();

        let mut buf = [0u8; 64];
        builder.write_to(&mut buf);

        // Self entry has identifier 0x00
        assert_eq!(buf[32], 1); // identifier length
        assert_eq!(buf[33], 0x00); // identifier (.)
    }

    #[test]
    fn test_parent_entry() {
        let builder = DirectoryRecordBuilder::new_parent_entry(50, 2048);
        let _size = builder.record_size();

        let mut buf = [0u8; 64];
        builder.write_to(&mut buf);

        // Parent entry has identifier 0x01
        assert_eq!(buf[32], 1); // identifier length
        assert_eq!(buf[33], 0x01); // identifier (..)
    }

    #[test]
    fn test_to_iso9660_name() {
        let name = to_iso9660_name("readme.txt", false);
        let len = iso9660_name_length(&name);
        assert_eq!(&name[..len as usize], b"README.TXT;1");

        let name = to_iso9660_name("subdir", true);
        let len = iso9660_name_length(&name);
        assert_eq!(&name[..len as usize], b"SUBDIR");
    }
}
