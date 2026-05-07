// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! ECMA-119 (ISO 9660) disk image parser.

mod directory;
pub(crate) mod parser;
mod path_table;

use core::fmt;
use core::mem::size_of;

pub use directory::{DirEntryIter, Directory, DirectoryEntry, File};
pub use path_table::PathTableIter;

use self::parser::Parser;
use crate::raw::{
    AStr, DStr, DecDateTime, DirectoryRecord, DirectoryRecordHeader, FileId, SECTOR_SIZE,
    VolumeDescriptorSet,
};

pub struct Image<'a> {
    pub(crate) data: &'a [u8],
    volume_descriptor_set: VolumeDescriptorSet<'a>,
    pub(crate) strict: bool,
}

impl fmt::Debug for Image<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("volume_descriptor_set", &self.volume_descriptor_set)
            .finish_non_exhaustive()
    }
}

impl<'a> Image<'a> {
    /// Parse an ISO image in **strict** mode (default).
    ///
    /// Every parsed field is validated against the ECMA-119 spec.  Use
    /// [`parse_lenient`](Self::parse_lenient) for images that bend the rules
    /// (e.g. real-world firmware images with non-compliant string encodings).
    ///
    /// # Errors
    ///
    /// Returns an error if the image is too small, contains invalid volume
    /// descriptors, or fails any ECMA-119 validation check.
    pub fn parse(data: &'a [u8]) -> anyhow::Result<Self> {
        Self::parse_inner(data, true)
    }

    /// Parse an ISO image in **relaxed** mode.
    ///
    /// Structural parsing still happens, but semantic validation is skipped.
    /// Prefer [`parse`](Self::parse) unless you need to read non-compliant
    /// images.
    ///
    /// # Errors
    ///
    /// Returns an error if the image is too small or structurally malformed.
    pub fn parse_relaxed(data: &'a [u8]) -> anyhow::Result<Self> {
        Self::parse_inner(data, false)
    }

    fn parse_inner(data: &'a [u8], strict: bool) -> anyhow::Result<Self> {
        let mut parser = if strict {
            Parser::new(data)
        } else {
            Parser::lenient(data)
        };

        // first come 16 sectors of "system area"
        let _system_area = parser.byte_array::<{ 16 * SECTOR_SIZE }>()?; // TODO properly parse this

        let volume_descriptor_set = parser.volume_descriptor_set()?;

        Ok(Self {
            data,
            volume_descriptor_set,
            strict,
        })
    }

    /// Returns the root directory.
    ///
    /// Always uses the primary volume descriptor's root.
    ///
    /// # Errors
    ///
    /// Returns an error if the root directory record is malformed.
    // TODO implement Joliet/Rock Ridge lookup (see ECMA-119 §§ 8.4,)
    //  1. Prefer the PVD when it carries Rock Ridge SUSP entries.
    //  2. Otherwise prefer a supplementary descriptor (e.g. Joliet).
    //  3. Otherwise fall back to the PVD with short filenames.
    pub fn root(&self) -> anyhow::Result<Directory<'_, 'a>> {
        let header = &self
            .volume_descriptor_set
            .primary
            .root_directory_record
            .header;

        let expected_len = size_of::<DirectoryRecordHeader>() + header.file_identifier_len as usize;
        anyhow::ensure!(
            header.len as usize == expected_len,
            "root directory record: len={} but expected {} (header={} + file_identifier_len={})",
            header.len,
            expected_len,
            size_of::<DirectoryRecordHeader>(),
            header.file_identifier_len,
        );

        Ok(Directory {
            img: self,
            record: DirectoryRecord {
                header,
                identifier: &[0x0],
                _system_use: &[],
            },
        })
    }

    pub fn system_id(&self) -> &AStr<32> {
        &self.volume_descriptor_set.primary.system_id
    }

    pub fn volume_id(&self) -> &DStr<32> {
        &self.volume_descriptor_set.primary.volume_id
    }

    pub fn volume_set_id(&self) -> &DStr<128> {
        &self.volume_descriptor_set.primary.volume_set_id
    }

    pub fn publisher_id(&self) -> &AStr<128> {
        &self.volume_descriptor_set.primary.publisher_id
    }

    pub fn data_preparer_id(&self) -> &AStr<128> {
        &self.volume_descriptor_set.primary.data_preparer_id
    }

    pub fn application_id(&self) -> &AStr<128> {
        &self.volume_descriptor_set.primary.application_id
    }

    pub fn copyright_file_identifier(&self) -> &FileId<37> {
        &self.volume_descriptor_set.primary.copyright_file_id
    }

    pub fn abstract_file_identifier(&self) -> &FileId<37> {
        &self.volume_descriptor_set.primary.abstract_file_id
    }

    pub fn bibliographic_file_identifier(&self) -> &FileId<37> {
        &self.volume_descriptor_set.primary.bibliographic_file_id
    }

    pub fn created_at(&self) -> &DecDateTime {
        &self.volume_descriptor_set.primary.creation_date
    }

    pub fn modified_at(&self) -> &DecDateTime {
        &self.volume_descriptor_set.primary.modification_date
    }

    pub fn expires_after(&self) -> &DecDateTime {
        &self.volume_descriptor_set.primary.expiration_date
    }

    pub fn effective_after(&self) -> &DecDateTime {
        &self.volume_descriptor_set.primary.effective_date
    }
}
