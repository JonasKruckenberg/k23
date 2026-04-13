use core::mem::size_of;
use core::str::{FromStr, Utf8Error};

use bitflags::bitflags;
use zerocopy::byteorder::{BigEndian, LittleEndian, U16, U32};
use zerocopy::{ByteOrder, FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::validate::{
    Validate, ValidationError, ValidationErrorKind, is_a_char, is_d_char, is_file_id_char,
};

pub const SECTOR_SIZE: usize = 2048;
/// El Torito uses 512-byte "virtual" sectors for boot image sector counts.
pub const VIRTUAL_SECTOR_SIZE: usize = 512;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BothEndianU16 {
    le: U16<LittleEndian>,
    be: U16<BigEndian>,
}
const _: () = assert!(size_of::<BothEndianU16>() == 4);

impl BothEndianU16 {
    pub fn new(n: u16) -> Self {
        Self {
            le: U16::new(n),
            be: U16::new(n),
        }
    }

    #[cfg(target_endian = "little")]
    pub fn get(self) -> u16 {
        self.le.get()
    }

    #[cfg(target_endian = "big")]
    pub fn get(self) -> u16 {
        self.be.get()
    }
}

impl Validate for BothEndianU16 {
    fn validate(&self) -> Result<(), ValidationError> {
        let le = self.le.get() as u64;
        let be = self.be.get() as u64;
        if le != be {
            return Err(ValidationError {
                path: "BothEndianU16",
                kind: ValidationErrorKind::EndianMismatch { le, be },
            });
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BothEndianU32 {
    le: U32<LittleEndian>,
    be: U32<BigEndian>,
}
const _: () = assert!(size_of::<BothEndianU32>() == 8);

impl BothEndianU32 {
    pub fn new(n: u32) -> Self {
        Self {
            le: U32::new(n),
            be: U32::new(n),
        }
    }

    #[cfg(target_endian = "little")]
    pub fn get(self) -> u32 {
        self.le.get()
    }

    #[cfg(target_endian = "big")]
    pub fn get(self) -> u32 {
        self.be.get()
    }
}

impl Validate for BothEndianU32 {
    fn validate(&self) -> Result<(), ValidationError> {
        let le = self.le.get() as u64;
        let be = self.be.get() as u64;
        if le != be {
            return Err(ValidationError {
                path: "BothEndianU32",
                kind: ValidationErrorKind::EndianMismatch { le, be },
            });
        }
        Ok(())
    }
}

// d-characters: A–Z, 0–9, `_` (ECMA-119 §7.4.1).
// Fields are padded with SPACE (`0x20`) to fill the fixed width.
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct DStr<const N: usize>([u8; N]);

impl<const N: usize> Validate for DStr<N> {
    fn validate(&self) -> Result<(), ValidationError> {
        // strings are padded to length with SPACE
        let bytes = self.0.trim_ascii_end();

        for (i, &b) in bytes.iter().enumerate() {
            if !is_d_char(b) && b != b' ' {
                return Err(ValidationError {
                    path: "DStr",
                    kind: ValidationErrorKind::BadCharacter {
                        byte: b,
                        position: i,
                    },
                });
            }
        }

        Ok(())
    }
}

impl<const N: usize> DStr<N> {
    pub fn try_from_bytes(bytes: [u8; N]) -> Result<Self, ValidationError> {
        let me = Self(bytes);
        me.validate()?;
        Ok(me)
    }

    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(&self.0)
    }
}

impl<const N: usize> FromStr for DStr<N> {
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() > N {
            return Err(ValidationError {
                path: "DStr",
                kind: ValidationErrorKind::LengthOutOfRange {
                    len: bytes.len(),
                    min: 0,
                    max: N,
                },
            });
        }

        let mut arr = [b' '; N];
        arr[..bytes.len()].copy_from_slice(bytes);

        Self::try_from_bytes(arr)
    }
}

// a-characters: d-characters plus SPACE and `! " % & ' ( ) * + , - . / : ; < = > ?`
// (ECMA-119 §7.4.2).
// Fields are padded with SPACE (`0x20`) to fill the fixed width.
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct AStr<const N: usize>([u8; N]);

impl<const N: usize> Validate for AStr<N> {
    fn validate(&self) -> Result<(), ValidationError> {
        // strings are padded to length with SPACE
        let bytes = self.0.trim_ascii_end();

        for (i, &b) in bytes.iter().enumerate() {
            if !is_a_char(b) {
                return Err(ValidationError {
                    path: "AStr",
                    kind: ValidationErrorKind::BadCharacter {
                        byte: b,
                        position: i,
                    },
                });
            }
        }

        Ok(())
    }
}

impl<const N: usize> AStr<N> {
    pub fn try_from_bytes(bytes: [u8; N]) -> Result<Self, ValidationError> {
        let me = Self(bytes);
        me.validate()?;
        Ok(me)
    }

    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(&self.0)
    }
}

impl<const N: usize> FromStr for AStr<N> {
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() > N {
            return Err(ValidationError {
                path: "AStr",
                kind: ValidationErrorKind::LengthOutOfRange {
                    len: bytes.len(),
                    min: 0,
                    max: N,
                },
            });
        }

        let mut arr = [b' '; N];
        arr[..bytes.len()].copy_from_slice(bytes);

        Self::try_from_bytes(arr)
    }
}

// d-characters: A–Z, 0–9, `_` (ECMA-119 §7.4.1).
// Fields are padded with SPACE (`0x20`) to fill the fixed width.
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct FileId<const N: usize>([u8; N]);

impl<const N: usize> Validate for FileId<N> {
    fn validate(&self) -> Result<(), ValidationError> {
        // strings are padded to length with SPACE
        let bytes = self.0.trim_ascii_end();

        for (i, &b) in bytes.iter().enumerate() {
            if !is_file_id_char(b) {
                return Err(ValidationError {
                    path: "FileId",
                    kind: ValidationErrorKind::BadCharacter {
                        byte: b,
                        position: i,
                    },
                });
            }
        }

        Ok(())
    }
}

impl<const N: usize> FileId<N> {
    pub fn try_from_bytes(bytes: [u8; N]) -> Result<Self, ValidationError> {
        let me = Self(bytes);
        me.validate()?;
        Ok(me)
    }

    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(&self.0)
    }
}

impl<const N: usize> FromStr for FileId<N> {
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() > N {
            return Err(ValidationError {
                path: "FileId",
                kind: ValidationErrorKind::LengthOutOfRange {
                    len: bytes.len(),
                    min: 0,
                    max: N,
                },
            });
        }

        let mut arr = [b' '; N];
        arr[..bytes.len()].copy_from_slice(bytes);

        Self::try_from_bytes(arr)
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct DecDateTime {
    pub year: DStr<4>,
    pub month: DStr<2>,
    pub day: DStr<2>,
    pub hour: DStr<2>,
    pub minute: DStr<2>,
    pub second: DStr<2>,
    pub hundreth: DStr<2>,
    pub timezone_offset: i8,
}
const _: () = assert!(size_of::<DecDateTime>() == 17);

impl DecDateTime {
    fn validate_field<const N: usize>(
        &self,
        field: &DStr<N>,
        min: u16,
        max: u16,
        expected: &'static str,
    ) -> Result<(), ValidationErrorKind> {
        let s = field.as_str().map_err(|_| ValidationErrorKind::BadValue {
            expected: "valid ASCII digits",
            found: format!("{:?}", field.0),
        })?;

        let num = u16::from_str(s.trim()).map_err(|_| ValidationErrorKind::BadValue {
            expected: "decimal digits",
            found: s.to_string(),
        })?;

        if num < min || num > max {
            return Err(ValidationErrorKind::BadValue {
                expected,
                found: num.to_string(),
            });
        }

        Ok(())
    }
}

impl Validate for DecDateTime {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.year.0 == [0, 0, 0, 0]
            && self.month.0 == [0, 0]
            && self.day.0 == [0, 0]
            && self.hour.0 == [0, 0]
            && self.minute.0 == [0, 0]
            && self.second.0 == [0, 0]
            && self.hundreth.0 == [0, 0]
            && self.timezone_offset == 0
        {
            return Ok(()); // signifies an absent date
        }

        self.validate_field(&self.year, 1, 9999, "ASCII digits between 1..=9999")
            .map_err(|kind| ValidationError {
                path: "DecDateTime.year",
                kind,
            })?;
        self.validate_field(&self.month, 1, 12, "ASCII digits between 1..=12")
            .map_err(|kind| ValidationError {
                path: "DecDateTime.month",
                kind,
            })?;
        self.validate_field(&self.day, 1, 31, "ASCII digits between 1..=31")
            .map_err(|kind| ValidationError {
                path: "DecDateTime.day",
                kind,
            })?;
        self.validate_field(&self.hour, 0, 23, "ASCII digits between 0..=23")
            .map_err(|kind| ValidationError {
                path: "DecDateTime.hour",
                kind,
            })?;
        self.validate_field(&self.minute, 0, 59, "ASCII digits between 0..=59")
            .map_err(|kind| ValidationError {
                path: "DecDateTime.minute",
                kind,
            })?;
        self.validate_field(&self.second, 0, 59, "ASCII digits between 0..=59")
            .map_err(|kind| ValidationError {
                path: "DecDateTime.second",
                kind,
            })?;
        self.validate_field(&self.hundreth, 0, 99, "ASCII digits between 0..=99")
            .map_err(|kind| ValidationError {
                path: "DecDateTime.hundreth",
                kind,
            })?;
        if self.timezone_offset < -48 || self.timezone_offset > 52 {
            return Err(ValidationError {
                path: "DecDateTime.timezone_offset",
                kind: ValidationErrorKind::BadValue {
                    expected: "ASCII digits between -48..=52",
                    found: self.timezone_offset.to_string(),
                },
            });
        }
        Ok(())
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct DirDateTime {
    pub year: u8,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub timezone_offset: i8,
}
const _: () = assert!(size_of::<DirDateTime>() == 7);

impl DirDateTime {
    fn validate_field(
        &self,
        field: u8,
        min: u8,
        max: u8,
        expected: &'static str,
    ) -> Result<(), ValidationErrorKind> {
        if field < min || field > max {
            Err(ValidationErrorKind::BadValue {
                expected,
                found: field.to_string(),
            })
        } else {
            Ok(())
        }
    }
}

impl Validate for DirDateTime {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.year == 0
            && self.month == 0
            && self.day == 0
            && self.hour == 0
            && self.minute == 0
            && self.second == 0
            && self.timezone_offset == 0
        {
            return Ok(()); // signifies an absent date
        }

        self.validate_field(self.month, 1, 12, "number between 1..=12")
            .map_err(|kind| ValidationError {
                path: "DirDateTime.month",
                kind,
            })?;
        self.validate_field(self.day, 1, 31, "number between 1..=32")
            .map_err(|kind| ValidationError {
                path: "DirDateTime.day",
                kind,
            })?;
        self.validate_field(self.hour, 0, 23, "number between 0..=23")
            .map_err(|kind| ValidationError {
                path: "DirDateTime.hour",
                kind,
            })?;
        self.validate_field(self.minute, 0, 59, "number between 0..=59")
            .map_err(|kind| ValidationError {
                path: "DirDateTime.minute",
                kind,
            })?;
        self.validate_field(self.second, 0, 59, "number between 0..=59")
            .map_err(|kind| ValidationError {
                path: "DirDateTime.second",
                kind,
            })?;
        if self.timezone_offset < -48 || self.timezone_offset > 52 {
            return Err(ValidationError {
                path: "DirDateTime.timezone_offset",
                kind: ValidationErrorKind::BadValue {
                    expected: "number between -48..=52",
                    found: self.timezone_offset.to_string(),
                },
            });
        }
        Ok(())
    }
}

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
}

impl Validate for DirectoryRecordHeader {
    fn validate(&self) -> Result<(), ValidationError> {
        let min_len = size_of::<DirectoryRecordHeader>() + self.file_identifier_len as usize;
        if (self.len as usize) < min_len {
            return Err(ValidationError {
                path: "DirectoryRecordHeader.len",
                kind: ValidationErrorKind::LengthOutOfRange {
                    len: self.len as usize,
                    min: min_len,
                    max: usize::MAX,
                },
            });
        }
        self.extent_lba
            .validate()
            .map_err(|e| e.at("DirectoryRecordHeader.extent_lba"))?;
        self.data_length
            .validate()
            .map_err(|e| e.at("DirectoryRecordHeader.data_length"))?;
        self.volume_sequence_number
            .validate()
            .map_err(|e| e.at("DirectoryRecordHeader.volume_sequence_number"))?;
        self.recording_date
            .validate()
            .map_err(|e| e.at("DirectoryRecordHeader.recording_date"))?;
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
    fn validate(&self) -> Result<(), ValidationError> {
        self.header
            .validate()
            .map_err(|e| e.at("RootDirectoryRecord.header"))?;

        if self.file_identifier != 0 {
            return Err(ValidationError {
                path: "RootDirectoryRecord.file_identifier",
                kind: ValidationErrorKind::BadValue {
                    expected: "0x0",
                    found: self.file_identifier.to_string(),
                },
            });
        }

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

#[derive(Debug)]
pub struct VolumeDescriptorSet<'a> {
    pub(crate) primary: &'a PrimaryVolumeDescriptor,
    pub(crate) boot: Vec<&'a BootRecord>,
    pub(crate) supplementary: Vec<&'a SupplementaryVolumeDescriptor>,
    pub(crate) enhanced: Vec<&'a EnhancedVolumeDescriptor>,
    pub(crate) volume_partition: Vec<&'a VolumePartitionDescriptor>,
}

// Not public: an implementation detail of the volume descriptor parser.
#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub(crate) struct VolumeDescriptorHeader {
    pub(crate) volume_descriptor_ty: u8,
    pub(crate) standard_id: [u8; 5],          // must be CD001
    pub(crate) volume_descriptor_version: u8, // must be 1
}

impl Validate for VolumeDescriptorHeader {
    fn validate(&self) -> Result<(), ValidationError> {
        if &self.standard_id != b"CD001" {
            return Err(ValidationError {
                path: "VolumeDescriptorHeader.standard_id",
                kind: ValidationErrorKind::BadMagic { expected: b"CD001" },
            });
        }
        if self.volume_descriptor_version != 1 {
            return Err(ValidationError {
                path: "VolumeDescriptorHeader.volume_descriptor_version",
                kind: ValidationErrorKind::BadValue {
                    expected: "0x1",
                    found: self.volume_descriptor_version.to_string(),
                },
            });
        }
        Ok(())
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct BootRecord {
    pub boot_system_id: AStr<32>, // must be "EL TORITO SPECIFICATION" padded with 0's.
    pub boot_id: AStr<32>,        // unused, must be zeroes
    /// Absolute pointer to first sector of Boot Catalog.
    pub(crate) boot_catalog: U32<LittleEndian>,
    pub system_use: [u8; 1973],
}
const _: () =
    assert!(size_of::<BootRecord>() + size_of::<VolumeDescriptorHeader>() - SECTOR_SIZE == 0);

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct PrimaryVolumeDescriptor {
    _reserved1: u8,
    pub system_id: AStr<32>,
    pub volume_id: DStr<32>,
    _reserved2: [u8; 8],
    pub volume_space_size: BothEndianU32,
    _reserved3: [u8; 32],
    pub volume_set_size: BothEndianU16,
    pub volume_sequence_number: BothEndianU16,
    pub logical_block_size: BothEndianU16,

    pub path_table_size: BothEndianU32,
    pub(crate) path_table_l_lba: U32<LittleEndian>,
    pub optional_path_table_l_lba: U32<LittleEndian>,
    pub(crate) path_table_m_lba: U32<BigEndian>,
    pub optional_path_table_m_lba: U32<BigEndian>,

    pub root_directory_record: RootDirectoryRecord,

    pub volume_set_id: DStr<128>,
    pub publisher_id: AStr<128>,
    pub data_preparer_id: AStr<128>,
    pub application_id: AStr<128>,

    pub copyright_file_id: FileId<37>,
    pub abstract_file_id: FileId<37>,
    pub bibliographic_file_id: FileId<37>,

    pub creation_date: DecDateTime,
    pub modification_date: DecDateTime,
    pub expiration_date: DecDateTime,
    pub effective_date: DecDateTime,

    pub file_structure_version: u8, // must be 1

    _reserved4: u8,
    pub system_use: [u8; 512],
    _reserved5: [u8; 653],
}
const _: () = assert!(
    size_of::<PrimaryVolumeDescriptor>() + size_of::<VolumeDescriptorHeader>() - SECTOR_SIZE == 0
);

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SupplementaryVolumeDescriptor {
    pub flags: u8,
    pub system_id: AStr<32>,
    pub volume_id: DStr<32>,
    _reserved1: [u8; 8],
    pub volume_space_size: BothEndianU32,
    pub escape_sequences: [u8; 32],
    pub volume_set_size: BothEndianU16,
    pub volume_sequence_number: BothEndianU16,
    pub logical_block_size: BothEndianU16,

    pub path_table_size: BothEndianU32,
    pub path_table_l_lba: U32<LittleEndian>,
    pub optional_path_table_l_lba: U32<LittleEndian>,
    pub path_table_m_lba: U32<BigEndian>,
    pub optional_path_table_m_lba: U32<BigEndian>,

    pub root_directory_record: RootDirectoryRecord,

    pub volume_set_id: DStr<128>,
    pub publisher_id: AStr<128>,
    pub data_preparer_id: AStr<128>,
    pub application_id: AStr<128>,

    pub copyright_file_id: FileId<37>,
    pub abstract_file_id: FileId<37>,
    pub bibliographic_file_id: FileId<37>,

    pub creation_date: DecDateTime,
    pub modification_date: DecDateTime,
    pub expiration_date: DecDateTime,
    pub effective_date: DecDateTime,

    pub file_structure_version: u8,

    _reserved2: u8,
    pub system_use: [u8; 512],
    _reserved3: [u8; 653],
}
const _: () = assert!(
    size_of::<SupplementaryVolumeDescriptor>() + size_of::<VolumeDescriptorHeader>() - SECTOR_SIZE
        == 0
);

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct EnhancedVolumeDescriptor {
    _reserved1: u8,
    pub system_id: AStr<32>,
    pub volume_id: DStr<32>,
    _reserved2: [u8; 8],
    pub volume_space_size: BothEndianU32,
    _reserved3: [u8; 32],
    pub volume_set_size: BothEndianU16,
    pub volume_sequence_number: BothEndianU16,
    pub logical_block_size: BothEndianU16,

    pub path_table_size: BothEndianU32,
    pub path_table_l_lba: U32<LittleEndian>,
    pub optional_path_table_l_lba: U32<LittleEndian>,
    pub path_table_m_lba: U32<BigEndian>,
    pub optional_path_table_m_lba: U32<BigEndian>,

    pub root_directory_record: RootDirectoryRecord,

    pub volume_set_id: [u8; 128],
    pub publisher_id: [u8; 128],
    pub data_preparer_id: [u8; 128],
    pub application_id: [u8; 128],

    pub copyright_file_id: [u8; 37],
    pub abstract_file_id: [u8; 37],
    pub bibliographic_file_id: [u8; 37],

    pub creation_date: DecDateTime,
    pub modification_date: DecDateTime,
    pub expiration_date: DecDateTime,
    pub effective_date: DecDateTime,

    pub file_structure_version: u8,

    _reserved4: u8,
    pub system_use: [u8; 512],
    _reserved5: [u8; 653],
}
const _: () = assert!(
    size_of::<EnhancedVolumeDescriptor>() + size_of::<VolumeDescriptorHeader>() - SECTOR_SIZE == 0
);

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct VolumePartitionDescriptor {
    _reserved1: u8,
    pub system_id: AStr<32>,
    pub partition_id: DStr<32>,
    pub partition_lba: BothEndianU32,
    pub partition_size: BothEndianU32,
    pub system_use: [u8; 1960],
}
const _: () = assert!(
    size_of::<VolumePartitionDescriptor>() + size_of::<VolumeDescriptorHeader>() - SECTOR_SIZE == 0
);

// ── Validate impls for volume descriptors ────────────────────────────────────

// const EL_TORITO_ID_STRICT: &[u8; 32] = b"EL TORITO SPECIFICATION\0\0\0\0\0\0\0\0\0";
const EL_TORITO_ID: &[u8] = b"EL TORITO SPECIFICATION";

impl Validate for BootRecord {
    fn validate(&self) -> Result<(), ValidationError> {
        if !self.boot_system_id.0.starts_with(EL_TORITO_ID) {
            return Err(ValidationError {
                path: "BootRecord.boot_system_id",
                kind: ValidationErrorKind::BadMagic {
                    expected: EL_TORITO_ID,
                },
            });
        }
        Ok(())
    }
}

impl Validate for PrimaryVolumeDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        self.system_id
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.system_id"))?;
        self.volume_id
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.volume_id"))?;
        self.volume_space_size
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.volume_space_size"))?;
        self.volume_set_size
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.volume_set_size"))?;
        self.volume_sequence_number
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.volume_sequence_number"))?;
        self.logical_block_size
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.logical_block_size"))?;
        self.path_table_size
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.path_table_size"))?;
        self.volume_set_id
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.volume_set_id"))?;
        self.publisher_id
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.publisher_id"))?;
        self.data_preparer_id
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.data_preparer_id"))?;
        self.application_id
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.application_id"))?;
        self.root_directory_record
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.root_directory_record"))?;
        self.creation_date
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.creation_date"))?;
        self.modification_date
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.modification_date"))?;
        self.expiration_date
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.expiration_date"))?;
        self.effective_date
            .validate()
            .map_err(|e| e.at("PrimaryVolumeDescriptor.effective_date"))?;
        if self.file_structure_version != 1 {
            return Err(ValidationError {
                path: "PrimaryVolumeDescriptor.file_structure_version",
                kind: ValidationErrorKind::BadValue {
                    expected: "0x1",
                    found: self.file_structure_version.to_string(),
                },
            });
        }
        Ok(())
    }
}

impl Validate for SupplementaryVolumeDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        self.system_id
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.system_id"))?;
        self.volume_id
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.volume_id"))?;
        self.volume_space_size
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.volume_space_size"))?;
        self.volume_set_size
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.volume_set_size"))?;
        self.volume_sequence_number
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.volume_sequence_number"))?;
        self.logical_block_size
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.logical_block_size"))?;
        self.path_table_size
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.path_table_size"))?;
        self.volume_set_id
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.volume_set_id"))?;
        self.publisher_id
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.publisher_id"))?;
        self.data_preparer_id
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.data_preparer_id"))?;
        self.application_id
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.application_id"))?;
        self.root_directory_record
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.root_directory_record"))?;
        self.creation_date
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.creation_date"))?;
        self.modification_date
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.modification_date"))?;
        self.expiration_date
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.expiration_date"))?;
        self.effective_date
            .validate()
            .map_err(|e| e.at("SupplementaryVolumeDescriptor.effective_date"))?;
        if self.file_structure_version != 1 {
            return Err(ValidationError {
                path: "SupplementaryVolumeDescriptor.file_structure_version",
                kind: ValidationErrorKind::BadValue {
                    expected: "0x1",
                    found: self.file_structure_version.to_string(),
                },
            });
        }
        Ok(())
    }
}

impl Validate for EnhancedVolumeDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        self.system_id
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.system_id"))?;
        self.volume_id
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.volume_id"))?;
        self.volume_space_size
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.volume_space_size"))?;
        self.volume_set_size
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.volume_set_size"))?;
        self.volume_sequence_number
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.volume_sequence_number"))?;
        self.logical_block_size
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.logical_block_size"))?;
        self.path_table_size
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.path_table_size"))?;
        self.root_directory_record
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.root_directory_record"))?;
        self.creation_date
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.creation_date"))?;
        self.modification_date
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.modification_date"))?;
        self.expiration_date
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.expiration_date"))?;
        self.effective_date
            .validate()
            .map_err(|e| e.at("EnhancedVolumeDescriptor.effective_date"))?;
        Ok(())
    }
}

impl Validate for VolumePartitionDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        self.system_id
            .validate()
            .map_err(|e| e.at("VolumePartitionDescriptor.system_id"))?;
        self.partition_id
            .validate()
            .map_err(|e| e.at("VolumePartitionDescriptor.partition_id"))?;
        self.partition_lba
            .validate()
            .map_err(|e| e.at("VolumePartitionDescriptor.partition_lba"))?;
        self.partition_size
            .validate()
            .map_err(|e| e.at("VolumePartitionDescriptor.partition_size"))?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum SystemUseEntry<'a> {
    ContinuationArea(&'a SystemUseEntryCE),    // "CE"
    SuspIndicator(&'a SystemUseEntrySP),       // "SP"
    SuspTerminator,                            // "ST"
    ExtensionsReference(SystemUseEntryER<'a>), // "ER"
    ExtensionSelector(&'a SystemUseEntryES),   // "ES"
    Unknown {
        header: &'a SystemUseEntryHeader,
        data: &'a [u8],
    },
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SystemUseEntryHeader {
    pub(crate) signature: [u8; 2], // ISO9660:7.1.1
    // length of the entry INCLUDING signature, length version and data
    pub(crate) len: u8,     // ISO9660:7.1.1
    pub(crate) version: u8, // ISO9660:7.1.1
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SystemUseEntryCE {
    header: SystemUseEntryHeader, // len = 28, version = 1
    block_location: BothEndianU32,
    offset: BothEndianU32,
    len: BothEndianU32,
}
const _: () = assert!(size_of::<SystemUseEntryCE>() == 28);

impl Validate for SystemUseEntryCE {
    fn validate(&self) -> Result<(), ValidationError> {
        assert!(&self.header.signature == b"CE");
        assert!(self.header.len == 28);
        assert!(self.header.version == 1);
        Ok(())
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SystemUseEntrySP {
    header: SystemUseEntryHeader, // len = 7, version = 1
    check_bytes: [u8; 2],         // [0xBE, 0xEF]
    // the number of bytes to be skipped within the system use field of each directory record (except the root).
    // before recording of system use entries other than the SP entries.
    pub(crate) bytes_skipped: u8,
}
const _: () = assert!(size_of::<SystemUseEntrySP>() == 7);

impl Validate for SystemUseEntrySP {
    fn validate(&self) -> Result<(), ValidationError> {
        assert!(&self.header.signature == b"SP");
        assert!(self.header.len == 7);
        assert!(self.header.version == 1);
        assert!(self.check_bytes == [0xBE, 0xEF]);
        Ok(())
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SystemUseEntryERHeader {
    header: SystemUseEntryHeader, // len = 8 + len id + len des + len src, version = 1
    pub(crate) identifier_len: u8,
    pub(crate) descriptor_len: u8,
    pub(crate) source_len: u8,
    pub(crate) extension_version: u8,
}
const _: () = assert!(size_of::<SystemUseEntryERHeader>() == 8);

impl Validate for SystemUseEntryERHeader {
    fn validate(&self) -> Result<(), ValidationError> {
        assert!(&self.header.signature == b"ER");
        assert!(self.header.version == 1);
        Ok(())
    }
}

#[derive(Debug)]
pub struct SystemUseEntryER<'a> {
    pub(crate) header: &'a SystemUseEntryERHeader,
    pub(crate) identifier: &'a [u8], // dstr
    pub(crate) descriptor: &'a [u8], // astr
    pub(crate) source: &'a [u8],     // astr
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SystemUseEntryES {
    header: SystemUseEntryHeader, // len = 5, version = 1
    sequence: u8,
}
const _: () = assert!(size_of::<SystemUseEntryES>() == 5);

impl Validate for SystemUseEntryES {
    fn validate(&self) -> Result<(), ValidationError> {
        assert!(&self.header.signature == b"ES");
        assert!(self.header.len == 7);
        assert!(self.header.version == 1);
        Ok(())
    }
}
