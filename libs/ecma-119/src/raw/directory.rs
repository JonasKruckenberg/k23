use core::mem::size_of;

use anyhow::Context as _;
use bitflags::bitflags;
use zerocopy::byteorder::{U16, U32};
use zerocopy::{ByteOrder, FromBytes, Immutable, IntoBytes, KnownLayout};

use super::both_endian::{BothEndianU16, BothEndianU32};
use super::datetime::DirDateTime;
use crate::validate::Validate;

bitflags! {
    #[derive(Debug, PartialEq, Eq)]
    pub struct FileFlags: u8 {
        /// If SET: the existence of this file need not be made known to the user (hidden, we ignore this)
        /// If UNSET: the existence of this file shall be made known to the user
        const EXISTENCE = 1 << 0;
        /// If SET: the record identifies a directory
        /// If UNSET: the record DOES NOT identify a directory
        const DIRECTORY = 1 << 1;
        /// If SET: the record identifies an associated file.
        /// If UNSET: the record DOES NOT identify an associated file.
        const ASSOCIATED_FILE = 1 << 2;
        /// If SET:  "the structure of the
        /// information in the file has a record format specified by
        /// a number other than zero in the record format field of
        /// the extended attribute record"
        ///
        /// If UNSET: "the structure of the
        /// information in the file is not specified by the record
        /// format field of any associated extended attribute record"
        const RECORD = 1 << 3;
        /// If SET: an owner and group are set for the record AND a permission bit in the associated extended attribute record is set
        /// If UNSET: NO owner or group are set for the record AND any user may read or execute the file
        const PROTECTION = 1 << 4;
        const _ = 1 << 5;
        const _ = 1 << 6;
        /// If SET: this is not the final directory record for the file.
        /// If UNSET: this IS the final directory record for the file.
        const MULTI_EXTENT = 1 << 7;
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct DirectoryRecordHeader {
    pub(crate) len: u8,
    pub extended_attribute_record_len: u8,
    pub extent_lba: BothEndianU32,
    pub data_length: BothEndianU32,
    pub recording_date: DirDateTime,
    pub flags: u8,
    pub interleaved_file_unit_size: u8,
    pub interleaved_gap_size: u8,
    pub volume_sequence_number: BothEndianU16,
    pub(crate) file_identifier_len: u8,
}
const _: () = assert!(size_of::<DirectoryRecordHeader>() == 33);

impl DirectoryRecordHeader {
    pub fn flags(&self) -> FileFlags {
        FileFlags::from_bits_truncate(self.flags)
    }

    /// Byte length of the padding byte inserted after the file identifier
    /// when `file_identifier_len` is even (ECMA-119 §9.1.12).
    pub(crate) fn file_identifier_pad(&self) -> usize {
        1 - (self.file_identifier_len as usize & 1)
    }

    /// Byte length of the System Use area that follows the identifier + pad.
    pub(crate) fn system_use_len(&self) -> usize {
        self.len as usize
            - size_of::<Self>()
            - self.file_identifier_len as usize
            - self.file_identifier_pad()
    }
}

impl Validate for DirectoryRecordHeader {
    fn validate(&self) -> anyhow::Result<()> {
        let min_len = size_of::<DirectoryRecordHeader>() + self.file_identifier_len as usize;
        anyhow::ensure!(
            self.len as usize >= min_len,
            "DirectoryRecordHeader.len: {} < minimum {min_len}",
            self.len,
        );
        self.extent_lba
            .validate()
            .context("DirectoryRecordHeader.extent_lba")?;
        self.data_length
            .validate()
            .context("DirectoryRecordHeader.data_length")?;
        self.volume_sequence_number
            .validate()
            .context("DirectoryRecordHeader.volume_sequence_number")?;
        self.recording_date
            .validate()
            .context("DirectoryRecordHeader.recording_date")?;
        Ok(())
    }
}

pub struct DirectoryRecord<'a> {
    pub(crate) header: &'a DirectoryRecordHeader,
    pub(crate) identifier: &'a [u8],
    pub(crate) system_use: &'a [u8],
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct RootDirectoryRecord {
    pub header: DirectoryRecordHeader,
    file_identifier: u8, // must be 0x0
}
const _: () = assert!(size_of::<RootDirectoryRecord>() == 34);

impl Validate for RootDirectoryRecord {
    fn validate(&self) -> anyhow::Result<()> {
        self.header
            .validate()
            .context("RootDirectoryRecord.header")?;
        anyhow::ensure!(
            self.file_identifier == 0,
            "RootDirectoryRecord.file_identifier: expected 0x0, got {:#04x}",
            self.file_identifier,
        );
        Ok(())
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct PathTableRecordHeader<O: ByteOrder> {
    pub(crate) len: u8,
    pub extended_attribute_record_len: u8,
    pub extent_lba: U32<O>,
    pub parent_directory: U16<O>,
}

#[derive(Debug)]
pub struct PathTableRecord<'a, O: ByteOrder> {
    pub header: &'a PathTableRecordHeader<O>,
    pub directory_id: &'a [u8],
}
