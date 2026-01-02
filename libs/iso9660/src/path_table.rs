// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Path Table structures for ISO 9660 filesystems.
//!
//! Path tables provide a fast way to locate directories without traversing
//! the directory hierarchy. Each entry contains the directory name, its
//! location, and the index of its parent directory.
//!
//! Two path tables are stored: one in little-endian format (Type L) and
//! one in big-endian format (Type M).
//!
//! Reference: ECMA-119 Section 9.4

extern crate alloc;

use alloc::vec::Vec;

/// Path Table Record.
///
/// Each path table record identifies a directory in the hierarchy.
/// The table is ordered by directory level, then alphabetically.
///
/// Reference: ECMA-119 Section 9.4
#[derive(Debug, Clone)]
pub struct PathTableRecord {
    /// Length of the directory identifier
    pub identifier_length: u8,
    /// Extended attribute record length
    pub extended_attribute_length: u8,
    /// Location of the directory extent
    pub location: u32,
    /// Directory number of the parent directory
    pub parent_directory_number: u16,
    /// Directory identifier
    pub identifier: [u8; 31],
}

impl PathTableRecord {
    /// Creates a new path table record.
    #[must_use]
    pub fn new(identifier: &str, location: u32, parent: u16) -> Self {
        let mut id_buf = [0u8; 31];
        let id_bytes = identifier.as_bytes();
        let id_len = id_bytes.len().min(31);
        id_buf[..id_len].copy_from_slice(&id_bytes[..id_len]);

        Self {
            identifier_length: id_len as u8,
            extended_attribute_length: 0,
            location,
            parent_directory_number: parent,
            identifier: id_buf,
        }
    }

    /// Creates a root directory path table record.
    #[must_use]
    pub fn root(location: u32) -> Self {
        Self {
            identifier_length: 1,
            extended_attribute_length: 0,
            location,
            parent_directory_number: 1,
            identifier: [0; 31], // Root has identifier of single 0x00 byte
        }
    }

    /// Returns the size of this record in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        // Base size: 1 + 1 + 4 + 2 = 8 bytes
        // Plus identifier length
        // Plus padding byte if identifier length is odd
        let base = 8 + self.identifier_length as usize;
        if self.identifier_length % 2 == 1 {
            base + 1
        } else {
            base
        }
    }

    /// Writes the record in little-endian format (Type L).
    ///
    /// Returns the number of bytes written.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is too small.
    pub fn write_le(&self, buf: &mut [u8]) -> usize {
        let size = self.size();
        assert!(buf.len() >= size, "buffer too small for path table record");

        buf[0] = self.identifier_length;
        buf[1] = self.extended_attribute_length;
        buf[2..6].copy_from_slice(&self.location.to_le_bytes());
        buf[6..8].copy_from_slice(&self.parent_directory_number.to_le_bytes());
        let id_len = self.identifier_length as usize;
        buf[8..8 + id_len].copy_from_slice(&self.identifier[..id_len]);

        // Add padding byte if needed
        if self.identifier_length % 2 == 1 {
            buf[8 + id_len] = 0;
        }

        size
    }

    /// Writes the record in big-endian format (Type M).
    ///
    /// Returns the number of bytes written.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is too small.
    pub fn write_be(&self, buf: &mut [u8]) -> usize {
        let size = self.size();
        assert!(buf.len() >= size, "buffer too small for path table record");

        buf[0] = self.identifier_length;
        buf[1] = self.extended_attribute_length;
        buf[2..6].copy_from_slice(&self.location.to_be_bytes());
        buf[6..8].copy_from_slice(&self.parent_directory_number.to_be_bytes());
        let id_len = self.identifier_length as usize;
        buf[8..8 + id_len].copy_from_slice(&self.identifier[..id_len]);

        // Add padding byte if needed
        if self.identifier_length % 2 == 1 {
            buf[8 + id_len] = 0;
        }

        size
    }
}

/// Builder for creating path tables.
///
/// Path tables must be built in directory level order, with directories
/// at the same level sorted alphabetically.
#[derive(Default)]
pub struct PathTableBuilder {
    records: Vec<PathTableRecord>,
}

impl PathTableBuilder {
    /// Creates a new path table builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Adds a directory to the path table.
    ///
    /// Returns the directory number (1-indexed) of the added directory.
    pub fn add_directory(&mut self, identifier: &str, location: u32, parent: u16) -> u16 {
        let dir_num = (self.records.len() + 1) as u16;
        self.records.push(PathTableRecord::new(identifier, location, parent));
        dir_num
    }

    /// Adds the root directory to the path table.
    ///
    /// This should be called first. Returns the directory number (1).
    pub fn add_root(&mut self, location: u32) -> u16 {
        self.records.push(PathTableRecord::root(location));
        1
    }

    /// Returns the total size of the path table in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.records.iter().map(PathTableRecord::size).sum()
    }

    /// Returns the number of records in the path table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns whether the path table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Writes the path table in little-endian format (Type L).
    ///
    /// Returns the number of bytes written.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is too small.
    pub fn write_le(&self, buf: &mut [u8]) -> usize {
        let mut offset = 0;
        for record in &self.records {
            offset += record.write_le(&mut buf[offset..]);
        }
        offset
    }

    /// Writes the path table in big-endian format (Type M).
    ///
    /// Returns the number of bytes written.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is too small.
    pub fn write_be(&self, buf: &mut [u8]) -> usize {
        let mut offset = 0;
        for record in &self.records {
            offset += record.write_be(&mut buf[offset..]);
        }
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_table_record_size() {
        // Root record: 8 + 1 (id) + 1 (padding) = 10
        let root = PathTableRecord::root(20);
        assert_eq!(root.size(), 10);

        // "TEST" = 4 chars: 8 + 4 = 12 (even, no padding)
        let test = PathTableRecord::new("TEST", 25, 1);
        assert_eq!(test.size(), 12);

        // "ABC" = 3 chars: 8 + 3 + 1 (padding) = 12
        let abc = PathTableRecord::new("ABC", 30, 1);
        assert_eq!(abc.size(), 12);
    }

    #[test]
    fn test_path_table_builder() {
        let mut builder = PathTableBuilder::new();
        let root_num = builder.add_root(20);
        assert_eq!(root_num, 1);

        let subdir_num = builder.add_directory("SUBDIR", 25, 1);
        assert_eq!(subdir_num, 2);

        assert_eq!(builder.len(), 2);
        assert!(builder.size() > 0);
    }

    #[test]
    fn test_write_le() {
        let record = PathTableRecord::root(20);
        let mut buf = [0u8; 16];
        let size = record.write_le(&mut buf);

        assert_eq!(size, 10);
        assert_eq!(buf[0], 1); // identifier length
        assert_eq!(buf[1], 0); // extended attribute length
        assert_eq!(&buf[2..6], &20u32.to_le_bytes()); // location
        assert_eq!(&buf[6..8], &1u16.to_le_bytes()); // parent
        assert_eq!(buf[8], 0); // root identifier
    }

    #[test]
    fn test_write_be() {
        let record = PathTableRecord::root(20);
        let mut buf = [0u8; 16];
        let size = record.write_be(&mut buf);

        assert_eq!(size, 10);
        assert_eq!(&buf[2..6], &20u32.to_be_bytes()); // location
        assert_eq!(&buf[6..8], &1u16.to_be_bytes()); // parent
    }
}
