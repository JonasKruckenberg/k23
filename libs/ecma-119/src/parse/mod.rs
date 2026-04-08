//! ECMA-119 (ISO 9660) disk image parser.

mod directory;
pub(crate) mod parser;
mod path_table;

use core::fmt;
use std::marker::PhantomData;
use std::mem::size_of;

pub use directory::{DirEntryIter, Directory, DirectoryEntry, File};
pub use path_table::PathTableIter;
use zerocopy::byteorder::{BigEndian, LittleEndian};

use self::parser::Parser;
use crate::raw::{
    AStr, BootRecord, DStr, DecDateTime, DirectoryRecord, DirectoryRecordHeader, FileId,
    SECTOR_SIZE, VolumeDescriptorSet,
};

#[derive(Debug)]
pub enum ParseError {}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl core::error::Error for ParseError {}

pub struct Image<'a> {
    pub(crate) data: &'a [u8],
    volume_descriptor_set: VolumeDescriptorSet<'a>,
}

impl fmt::Debug for Image<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("volume_descriptor_set", &self.volume_descriptor_set)
            .finish_non_exhaustive()
    }
}

impl<'a> Image<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, ParseError> {
        let mut parser = Parser { data, pos: 0 };

        // first come 16 sectors of "system area"
        let _system_area = parser.byte_array::<{ 16 * SECTOR_SIZE }>()?; // TODO properly parse this

        let volume_descriptor_set = parser.volume_descriptor_set()?;

        Ok(Self {
            data,
            volume_descriptor_set,
        })
    }

    /// Returns the root directory
    pub fn root(&self) -> Directory<'_, 'a> {
        // # Root selection
        // * If the primary volume descriptor has Rock Ridge SUSP entries, use it
        // * ElseIf a supplementary volume descriptor (e.g. Joliet) exists, use it
        // * Else fall back on the primary volume descriptor with short filenames

        // # See Also
        // ISO-9660 / ECMA-119 §§ 8.4, 8.5

        let header = &self
            .volume_descriptor_set
            .primary
            .root_directory_record
            .header;

        assert_eq!(
            header.len as usize
                - size_of::<DirectoryRecordHeader>()
                - header.file_identifier_len as usize,
            0
        );

        Directory {
            img: self,
            record: DirectoryRecord {
                header,
                identifier: &[0x0],
                system_use: &[],
            },
        }
    }

    /// Returns the root directory
    pub fn root_at(&self, index: usize) -> Option<Directory<'_, 'a>> {
        todo!()
    }

    /// Open a directory entry at a given path
    pub fn open(&self, path: &str) -> Result<Option<DirectoryEntry<'_, 'a>>, ParseError> {
        // self.root().find_recursive(path)
        todo!()
    }

    pub fn volume_set_identifier(&self) -> &DStr<128> {
        &self.volume_descriptor_set.primary.volume_set_id
    }

    pub fn publisher_identifier(&self) -> &AStr<128> {
        &self.volume_descriptor_set.primary.publisher_id
    }

    pub fn data_preparer_identifier(&self) -> &AStr<128> {
        &self.volume_descriptor_set.primary.data_preparer_id
    }

    pub fn application_identifier(&self) -> &AStr<128> {
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

    pub fn _path_table_le(&self) -> Result<PathTableIter<'a, LittleEndian>, ParseError> {
        let lba = self.volume_descriptor_set.primary.path_table_l_lba.get();
        let len = self.volume_descriptor_set.primary.path_table_size.get();

        let parser = Parser::from_lba_and_len(self.data, lba, len)?;
        Ok(PathTableIter {
            parser,
            endianness: PhantomData,
        })
    }

    pub fn _path_table_be(&self) -> Result<PathTableIter<'a, BigEndian>, ParseError> {
        let lba = self.volume_descriptor_set.primary.path_table_m_lba.get();
        let len = self.volume_descriptor_set.primary.path_table_size.get();

        let parser = Parser::from_lba_and_len(self.data, lba, len)?;
        Ok(PathTableIter {
            parser,
            endianness: PhantomData,
        })
    }

    pub fn _boot_records(&self) -> impl ExactSizeIterator<Item = &'a BootRecord> {
        self.volume_descriptor_set.boot.iter().copied()
    }
}
