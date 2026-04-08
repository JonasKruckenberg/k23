use std::mem::size_of;

use bitflags::bitflags;
use zerocopy::byteorder::{BigEndian, LittleEndian, U16, U32};
use zerocopy::{ByteOrder, FromBytes, Immutable, IntoBytes, KnownLayout};

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

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct AStr<const N: usize>(pub [u8; N]);

impl<const N: usize> AStr<N> {
    pub(crate) fn from_bytes(bytes: [u8; N]) -> Self {
        Self(bytes)
    }
}

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct DStr<const N: usize>(pub [u8; N]);

impl<const N: usize> DStr<N> {
    pub(crate) fn from_bytes(bytes: [u8; N]) -> Self {
        Self(bytes)
    }
}

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct FileId<const N: usize>(pub [u8; N]);

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
        FileFlags::from_bits(self.flags).unwrap()
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
