use core::mem::size_of;

use anyhow::Context as _;
use zerocopy::byteorder::{BigEndian, LittleEndian, U32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use super::SECTOR_SIZE;
use super::both_endian::{BothEndianU16, BothEndianU32};
use super::datetime::DecDateTime;
use super::directory::RootDirectoryRecord;
use super::str::{AStr, DStr, FileId};
use crate::validate::Validate;

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
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            &self.standard_id == b"CD001",
            "VolumeDescriptorHeader.standard_id: expected b\"CD001\", got {:?}",
            self.standard_id,
        );
        anyhow::ensure!(
            matches!(self.volume_descriptor_version, 1 | 2),
            "VolumeDescriptorHeader.volume_descriptor_version: expected 1 or 2, got {}",
            self.volume_descriptor_version,
        );
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
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.boot_system_id.0.starts_with(EL_TORITO_ID),
            "BootRecord.boot_system_id: missing \"EL TORITO SPECIFICATION\" prefix",
        );
        Ok(())
    }
}

impl Validate for PrimaryVolumeDescriptor {
    fn validate(&self) -> anyhow::Result<()> {
        self.system_id
            .validate()
            .context("PrimaryVolumeDescriptor.system_id")?;
        self.volume_id
            .validate()
            .context("PrimaryVolumeDescriptor.volume_id")?;
        self.volume_space_size
            .validate()
            .context("PrimaryVolumeDescriptor.volume_space_size")?;
        self.volume_set_size
            .validate()
            .context("PrimaryVolumeDescriptor.volume_set_size")?;
        self.volume_sequence_number
            .validate()
            .context("PrimaryVolumeDescriptor.volume_sequence_number")?;
        self.logical_block_size
            .validate()
            .context("PrimaryVolumeDescriptor.logical_block_size")?;
        self.path_table_size
            .validate()
            .context("PrimaryVolumeDescriptor.path_table_size")?;
        self.volume_set_id
            .validate()
            .context("PrimaryVolumeDescriptor.volume_set_id")?;
        self.publisher_id
            .validate()
            .context("PrimaryVolumeDescriptor.publisher_id")?;
        self.data_preparer_id
            .validate()
            .context("PrimaryVolumeDescriptor.data_preparer_id")?;
        self.application_id
            .validate()
            .context("PrimaryVolumeDescriptor.application_id")?;
        self.root_directory_record
            .validate()
            .context("PrimaryVolumeDescriptor.root_directory_record")?;
        self.creation_date
            .validate()
            .context("PrimaryVolumeDescriptor.creation_date")?;
        self.modification_date
            .validate()
            .context("PrimaryVolumeDescriptor.modification_date")?;
        self.expiration_date
            .validate()
            .context("PrimaryVolumeDescriptor.expiration_date")?;
        self.effective_date
            .validate()
            .context("PrimaryVolumeDescriptor.effective_date")?;
        anyhow::ensure!(
            self.file_structure_version == 1,
            "PrimaryVolumeDescriptor.file_structure_version: expected 1, got {}",
            self.file_structure_version,
        );
        Ok(())
    }
}

impl Validate for SupplementaryVolumeDescriptor {
    fn validate(&self) -> anyhow::Result<()> {
        self.system_id
            .validate()
            .context("SupplementaryVolumeDescriptor.system_id")?;
        self.volume_id
            .validate()
            .context("SupplementaryVolumeDescriptor.volume_id")?;
        self.volume_space_size
            .validate()
            .context("SupplementaryVolumeDescriptor.volume_space_size")?;
        self.volume_set_size
            .validate()
            .context("SupplementaryVolumeDescriptor.volume_set_size")?;
        self.volume_sequence_number
            .validate()
            .context("SupplementaryVolumeDescriptor.volume_sequence_number")?;
        self.logical_block_size
            .validate()
            .context("SupplementaryVolumeDescriptor.logical_block_size")?;
        self.path_table_size
            .validate()
            .context("SupplementaryVolumeDescriptor.path_table_size")?;
        self.volume_set_id
            .validate()
            .context("SupplementaryVolumeDescriptor.volume_set_id")?;
        self.publisher_id
            .validate()
            .context("SupplementaryVolumeDescriptor.publisher_id")?;
        self.data_preparer_id
            .validate()
            .context("SupplementaryVolumeDescriptor.data_preparer_id")?;
        self.application_id
            .validate()
            .context("SupplementaryVolumeDescriptor.application_id")?;
        self.root_directory_record
            .validate()
            .context("SupplementaryVolumeDescriptor.root_directory_record")?;
        self.creation_date
            .validate()
            .context("SupplementaryVolumeDescriptor.creation_date")?;
        self.modification_date
            .validate()
            .context("SupplementaryVolumeDescriptor.modification_date")?;
        self.expiration_date
            .validate()
            .context("SupplementaryVolumeDescriptor.expiration_date")?;
        self.effective_date
            .validate()
            .context("SupplementaryVolumeDescriptor.effective_date")?;
        anyhow::ensure!(
            self.file_structure_version == 1,
            "SupplementaryVolumeDescriptor.file_structure_version: expected 1, got {}",
            self.file_structure_version,
        );
        Ok(())
    }
}

impl Validate for EnhancedVolumeDescriptor {
    fn validate(&self) -> anyhow::Result<()> {
        self.system_id
            .validate()
            .context("EnhancedVolumeDescriptor.system_id")?;
        self.volume_id
            .validate()
            .context("EnhancedVolumeDescriptor.volume_id")?;
        self.volume_space_size
            .validate()
            .context("EnhancedVolumeDescriptor.volume_space_size")?;
        self.volume_set_size
            .validate()
            .context("EnhancedVolumeDescriptor.volume_set_size")?;
        self.volume_sequence_number
            .validate()
            .context("EnhancedVolumeDescriptor.volume_sequence_number")?;
        self.logical_block_size
            .validate()
            .context("EnhancedVolumeDescriptor.logical_block_size")?;
        self.path_table_size
            .validate()
            .context("EnhancedVolumeDescriptor.path_table_size")?;
        self.root_directory_record
            .validate()
            .context("EnhancedVolumeDescriptor.root_directory_record")?;
        self.creation_date
            .validate()
            .context("EnhancedVolumeDescriptor.creation_date")?;
        self.modification_date
            .validate()
            .context("EnhancedVolumeDescriptor.modification_date")?;
        self.expiration_date
            .validate()
            .context("EnhancedVolumeDescriptor.expiration_date")?;
        self.effective_date
            .validate()
            .context("EnhancedVolumeDescriptor.effective_date")?;
        Ok(())
    }
}

impl Validate for VolumePartitionDescriptor {
    fn validate(&self) -> anyhow::Result<()> {
        self.system_id
            .validate()
            .context("VolumePartitionDescriptor.system_id")?;
        self.partition_id
            .validate()
            .context("VolumePartitionDescriptor.partition_id")?;
        self.partition_lba
            .validate()
            .context("VolumePartitionDescriptor.partition_lba")?;
        self.partition_size
            .validate()
            .context("VolumePartitionDescriptor.partition_size")?;
        Ok(())
    }
}
