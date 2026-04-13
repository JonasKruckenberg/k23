//! ECMA-119 (ISO 9660) disk image parser.

mod directory;
pub(crate) mod parser;
mod path_table;
mod rock_ridge;
mod susp;

use core::fmt;
use core::mem::size_of;

pub use directory::{DirEntryIter, Directory, DirectoryEntry, File};
pub use path_table::PathTableIter;

use self::parser::Parser;
use self::susp::SystemUseIter;
use crate::raw::{
    AStr, DStr, DecDateTime, DirectoryRecord, DirectoryRecordHeader, FileId, SECTOR_SIZE,
    SystemUseEntry, VolumeDescriptorSet,
};
use crate::validate::ValidationError;

#[derive(Debug)]
pub enum ParseError {
    /// A field failed semantic validation (strict mode only).
    Invalid(ValidationError),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Invalid(e) => write!(f, "invalid field: {e}"),
        }
    }
}

impl core::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            ParseError::Invalid(e) => Some(e),
        }
    }
}

pub struct Image<'a> {
    pub(crate) data: &'a [u8],
    volume_descriptor_set: VolumeDescriptorSet<'a>,
    pub(crate) strict: bool,
    /// `None` means the image carries no SUSP entries.
    /// `Some(n)` means SUSP is present and each directory record's system use
    /// area begins with `n` bytes that must be skipped before SUSP entries.
    pub(crate) susp_skip: Option<u8>,
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
    pub fn parse(data: &'a [u8]) -> Result<Self, ParseError> {
        Self::parse_inner(data, true)
    }

    /// Parse an ISO image in **relaxed** mode.
    ///
    /// Structural parsing still happens, but semantic validation is skipped.
    /// Prefer [`parse`](Self::parse) unless you need to read non-compliant
    /// images.
    pub fn parse_relaxed(data: &'a [u8]) -> Result<Self, ParseError> {
        Self::parse_inner(data, false)
    }

    fn parse_inner(data: &'a [u8], strict: bool) -> Result<Self, ParseError> {
        fn detect_susp<'a>(
            data: &'a [u8],
            strict: bool,
            vds: &VolumeDescriptorSet<'a>,
        ) -> Result<Option<u8>, ParseError> {
            // SUSP §6.3: the SP entry must appear in the system use field of
            // the first directory record ("." entry) of the root directory.
            // We read that record directly — no skip is applied to the root ".".
            let root_header = &vds.primary.root_directory_record.header;
            let dir_data = parser::lba_to_slice(
                data,
                root_header.extent_lba.get(),
                root_header.data_length.get(),
            )?;

            let mut p = if strict {
                Parser::new(dir_data)
            } else {
                Parser::lenient(dir_data)
            };

            let entry_header = p.read_validated::<DirectoryRecordHeader>()?;
            p.bytes(entry_header.file_identifier_len as usize)?;
            // Skip the padding byte (§9.1.12) if LEN_FI is even
            let pad = 1 - (entry_header.file_identifier_len as usize & 1);
            p.bytes(pad)?;
            let system_use_len = entry_header.len as usize
                - size_of::<DirectoryRecordHeader>()
                - entry_header.file_identifier_len as usize
                - pad;
            let system_use = p.bytes(system_use_len)?;

            let mut iter = SystemUseIter {
                parser: Parser {
                    data: system_use,
                    pos: 0,
                    strict,
                },
                done: false,
            };

            while let Some(entry) = fallible_iterator::FallibleIterator::next(&mut iter)? {
                if let SystemUseEntry::SuspIndicator(sp) = entry {
                    return Ok(Some(sp.bytes_skipped));
                }
            }

            Ok(None)
        }

        let mut parser = if strict {
            Parser::new(data)
        } else {
            Parser::lenient(data)
        };

        // first come 16 sectors of "system area"
        let _system_area = parser.byte_array::<{ 16 * SECTOR_SIZE }>()?; // TODO properly parse this

        let volume_descriptor_set = parser.volume_descriptor_set()?;
        let susp_skip = detect_susp(data, strict, &volume_descriptor_set)?;

        Ok(Self {
            data,
            volume_descriptor_set,
            strict,
            susp_skip,
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

    pub fn system_identifier(&self) -> &AStr<32> {
        &self.volume_descriptor_set.primary.system_id
    }

    pub fn volume_identifier(&self) -> &DStr<32> {
        &self.volume_descriptor_set.primary.volume_id
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

    // pub fn _path_table_le(&self) -> Result<PathTableIter<'a, LittleEndian>, ParseError> {
    //     let lba = self.volume_descriptor_set.primary.path_table_l_lba.get();
    //     let len = self.volume_descriptor_set.primary.path_table_size.get();

    //     let parser = Parser::from_lba_and_len(self.data, lba, len, self.strict)?;
    //     Ok(PathTableIter {
    //         parser,
    //         endianness: PhantomData,
    //     })
    // }

    // pub fn _path_table_be(&self) -> Result<PathTableIter<'a, BigEndian>, ParseError> {
    //     let lba = self.volume_descriptor_set.primary.path_table_m_lba.get();
    //     let len = self.volume_descriptor_set.primary.path_table_size.get();

    //     let parser = Parser::from_lba_and_len(self.data, lba, len, self.strict)?;
    //     Ok(PathTableIter {
    //         parser,
    //         endianness: PhantomData,
    //     })
    // }

    // pub fn _boot_records(&self) -> impl ExactSizeIterator<Item = &'a BootRecord> {
    //     self.volume_descriptor_set.boot.iter().copied()
    // }
}
