// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Volume Descriptor structures for ISO 9660 filesystems.
//!
//! Volume descriptors are located starting at sector 16 (after the 32KB system area).
//! Each descriptor is exactly 2048 bytes (one sector).
//!
//! The volume descriptor set must contain:
//! - At least one Primary Volume Descriptor (type 1)
//! - A Volume Descriptor Set Terminator (type 255)
//!
//! For bootable ISOs using El Torito, a Boot Record (type 0) must also be present.

use crate::directory::DirectoryRecord;
use crate::types::{BothEndian16, BothEndian32, StrA, VolumeDateTime, SECTOR_SIZE};

/// Standard identifier for ISO 9660 volume descriptors.
pub const STANDARD_IDENTIFIER: &[u8; 5] = b"CD001";

/// Volume descriptor type codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VolumeDescriptorType {
    /// Boot Record (El Torito)
    BootRecord = 0,
    /// Primary Volume Descriptor
    Primary = 1,
    /// Supplementary Volume Descriptor (Joliet)
    Supplementary = 2,
    /// Volume Partition Descriptor
    Partition = 3,
    /// Volume Descriptor Set Terminator
    Terminator = 255,
}

/// Primary Volume Descriptor (PVD).
///
/// The PVD contains essential information about the ISO 9660 volume including
/// identifiers, size, and pointers to the root directory and path tables.
///
/// Located at sector 16 (byte offset 32768).
///
/// Reference: ECMA-119 Section 8.4
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct PrimaryVolumeDescriptor {
    /// Volume Descriptor Type (1 for Primary)
    pub type_code: u8,
    /// Standard Identifier ("CD001")
    pub standard_identifier: [u8; 5],
    /// Volume Descriptor Version (1)
    pub version: u8,
    /// Unused field (0)
    pub unused1: u8,
    /// System Identifier (32 bytes, a-characters)
    pub system_identifier: StrA<32>,
    /// Volume Identifier (32 bytes, d-characters)
    pub volume_identifier: StrA<32>,
    /// Unused field (zeros)
    pub unused2: [u8; 8],
    /// Volume Space Size (number of logical blocks)
    pub volume_space_size: BothEndian32,
    /// Unused field (zeros)
    pub unused3: [u8; 32],
    /// Volume Set Size (typically 1)
    pub volume_set_size: BothEndian16,
    /// Volume Sequence Number (typically 1)
    pub volume_sequence_number: BothEndian16,
    /// Logical Block Size (2048)
    pub logical_block_size: BothEndian16,
    /// Path Table Size in bytes
    pub path_table_size: BothEndian32,
    /// Location of Type L Path Table (little-endian)
    pub type_l_path_table_location: u32,
    /// Location of Optional Type L Path Table (0 if not present)
    pub optional_type_l_path_table_location: u32,
    /// Location of Type M Path Table (big-endian)
    pub type_m_path_table_location: u32,
    /// Location of Optional Type M Path Table (0 if not present)
    pub optional_type_m_path_table_location: u32,
    /// Root Directory Record (34 bytes)
    pub root_directory_record: DirectoryRecord,
    /// Volume Set Identifier (128 bytes)
    pub volume_set_identifier: StrA<128>,
    /// Publisher Identifier (128 bytes)
    pub publisher_identifier: StrA<128>,
    /// Data Preparer Identifier (128 bytes)
    pub data_preparer_identifier: StrA<128>,
    /// Application Identifier (128 bytes)
    pub application_identifier: StrA<128>,
    /// Copyright File Identifier (37 bytes)
    pub copyright_file_identifier: StrA<37>,
    /// Abstract File Identifier (37 bytes)
    pub abstract_file_identifier: StrA<37>,
    /// Bibliographic File Identifier (37 bytes)
    pub bibliographic_file_identifier: StrA<37>,
    /// Volume Creation Date and Time
    pub volume_creation_date: VolumeDateTime,
    /// Volume Modification Date and Time
    pub volume_modification_date: VolumeDateTime,
    /// Volume Expiration Date and Time
    pub volume_expiration_date: VolumeDateTime,
    /// Volume Effective Date and Time
    pub volume_effective_date: VolumeDateTime,
    /// File Structure Version (1)
    pub file_structure_version: u8,
    /// Reserved (0)
    pub reserved1: u8,
    /// Application Use (512 bytes)
    pub application_use: [u8; 512],
    /// Reserved (653 bytes)
    pub reserved2: [u8; 653],
}

impl PrimaryVolumeDescriptor {
    /// Creates a new Primary Volume Descriptor with default values.
    #[must_use]
    pub fn new() -> Self {
        let mut pvd = Self {
            type_code: VolumeDescriptorType::Primary as u8,
            standard_identifier: *STANDARD_IDENTIFIER,
            version: 1,
            unused1: 0,
            system_identifier: StrA::empty(),
            volume_identifier: StrA::empty(),
            unused2: [0; 8],
            volume_space_size: BothEndian32::new(0),
            unused3: [0; 32],
            volume_set_size: BothEndian16::new(1),
            volume_sequence_number: BothEndian16::new(1),
            logical_block_size: BothEndian16::new(SECTOR_SIZE as u16),
            path_table_size: BothEndian32::new(0),
            type_l_path_table_location: 0,
            optional_type_l_path_table_location: 0,
            type_m_path_table_location: 0,
            optional_type_m_path_table_location: 0,
            root_directory_record: DirectoryRecord::new_root(),
            volume_set_identifier: StrA::empty(),
            publisher_identifier: StrA::empty(),
            data_preparer_identifier: StrA::empty(),
            application_identifier: StrA::empty(),
            copyright_file_identifier: StrA::empty(),
            abstract_file_identifier: StrA::empty(),
            bibliographic_file_identifier: StrA::empty(),
            volume_creation_date: VolumeDateTime::unspecified(),
            volume_modification_date: VolumeDateTime::unspecified(),
            volume_expiration_date: VolumeDateTime::unspecified(),
            volume_effective_date: VolumeDateTime::unspecified(),
            file_structure_version: 1,
            reserved1: 0,
            application_use: [0; 512],
            reserved2: [0; 653],
        };

        // Set a default application identifier
        pvd.application_identifier = StrA::from_str("ISO9660 RUST LIBRARY");

        pvd
    }

    /// Sets the volume identifier.
    pub fn set_volume_identifier(&mut self, identifier: &str) {
        self.volume_identifier = StrA::from_str(identifier);
    }

    /// Sets the system identifier.
    pub fn set_system_identifier(&mut self, identifier: &str) {
        self.system_identifier = StrA::from_str(identifier);
    }

    /// Sets the publisher identifier.
    pub fn set_publisher_identifier(&mut self, identifier: &str) {
        self.publisher_identifier = StrA::from_str(identifier);
    }

    /// Sets the data preparer identifier.
    pub fn set_data_preparer_identifier(&mut self, identifier: &str) {
        self.data_preparer_identifier = StrA::from_str(identifier);
    }

    /// Sets the application identifier.
    pub fn set_application_identifier(&mut self, identifier: &str) {
        self.application_identifier = StrA::from_str(identifier);
    }

    /// Sets the volume space size in sectors.
    pub fn set_volume_space_size(&mut self, sectors: u32) {
        self.volume_space_size = BothEndian32::new(sectors);
    }

    /// Sets the path table information.
    pub fn set_path_table(
        &mut self,
        size: u32,
        type_l_location: u32,
        type_m_location: u32,
    ) {
        self.path_table_size = BothEndian32::new(size);
        self.type_l_path_table_location = type_l_location.to_le();
        self.type_m_path_table_location = type_m_location.to_be();
    }

    /// Sets the root directory record location and size.
    pub fn set_root_directory(&mut self, location: u32, size: u32) {
        self.root_directory_record.location = BothEndian32::new(location);
        self.root_directory_record.data_length = BothEndian32::new(size);
    }

    /// Sets the volume creation date.
    pub fn set_creation_date(&mut self, date: VolumeDateTime) {
        self.volume_creation_date = date;
        self.volume_modification_date = date;
    }

    /// Serializes the descriptor to a sector-sized buffer.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; SECTOR_SIZE] {
        // SAFETY: PrimaryVolumeDescriptor is repr(C, packed) and exactly SECTOR_SIZE bytes
        unsafe { &*(self as *const Self as *const [u8; SECTOR_SIZE]) }
    }
}

impl Default for PrimaryVolumeDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

const _: () = assert!(size_of::<PrimaryVolumeDescriptor>() == SECTOR_SIZE);

/// Boot Record Volume Descriptor for El Torito.
///
/// The Boot Record contains the identifier "EL TORITO SPECIFICATION" and
/// a pointer to the Boot Catalog.
///
/// Reference: El Torito Specification Section 2.0
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct BootRecordVolumeDescriptor {
    /// Volume Descriptor Type (0 for Boot Record)
    pub type_code: u8,
    /// Standard Identifier ("CD001")
    pub standard_identifier: [u8; 5],
    /// Version (1)
    pub version: u8,
    /// Boot System Identifier ("EL TORITO SPECIFICATION")
    pub boot_system_identifier: [u8; 32],
    /// Unused (zeros)
    pub unused: [u8; 32],
    /// Absolute sector number of the Boot Catalog
    pub boot_catalog_location: u32,
    /// Reserved (zeros)
    pub reserved: [u8; 1973],
}

impl BootRecordVolumeDescriptor {
    /// El Torito boot system identifier string.
    pub const BOOT_SYSTEM_ID: &[u8; 23] = b"EL TORITO SPECIFICATION";

    /// Creates a new Boot Record Volume Descriptor.
    #[must_use]
    pub fn new(boot_catalog_location: u32) -> Self {
        let mut boot_system_identifier = [0u8; 32];
        boot_system_identifier[..23].copy_from_slice(Self::BOOT_SYSTEM_ID);

        Self {
            type_code: VolumeDescriptorType::BootRecord as u8,
            standard_identifier: *STANDARD_IDENTIFIER,
            version: 1,
            boot_system_identifier,
            unused: [0; 32],
            boot_catalog_location: boot_catalog_location.to_le(),
            reserved: [0; 1973],
        }
    }

    /// Returns the boot catalog location.
    #[must_use]
    pub fn boot_catalog_location(&self) -> u32 {
        u32::from_le(self.boot_catalog_location)
    }

    /// Serializes the descriptor to a sector-sized buffer.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; SECTOR_SIZE] {
        // SAFETY: BootRecordVolumeDescriptor is repr(C, packed) and exactly SECTOR_SIZE bytes
        unsafe { &*(self as *const Self as *const [u8; SECTOR_SIZE]) }
    }
}

const _: () = assert!(size_of::<BootRecordVolumeDescriptor>() == SECTOR_SIZE);

/// Volume Descriptor Set Terminator.
///
/// Marks the end of the volume descriptor set. Must be the last volume descriptor.
///
/// Reference: ECMA-119 Section 8.3
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct VolumeDescriptorSetTerminator {
    /// Volume Descriptor Type (255 for Terminator)
    pub type_code: u8,
    /// Standard Identifier ("CD001")
    pub standard_identifier: [u8; 5],
    /// Version (1)
    pub version: u8,
    /// Reserved (zeros)
    pub reserved: [u8; 2041],
}

impl VolumeDescriptorSetTerminator {
    /// Creates a new Volume Descriptor Set Terminator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            type_code: VolumeDescriptorType::Terminator as u8,
            standard_identifier: *STANDARD_IDENTIFIER,
            version: 1,
            reserved: [0; 2041],
        }
    }

    /// Serializes the descriptor to a sector-sized buffer.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; SECTOR_SIZE] {
        // SAFETY: VolumeDescriptorSetTerminator is repr(C, packed) and exactly SECTOR_SIZE bytes
        unsafe { &*(self as *const Self as *const [u8; SECTOR_SIZE]) }
    }
}

impl Default for VolumeDescriptorSetTerminator {
    fn default() -> Self {
        Self::new()
    }
}

const _: () = assert!(size_of::<VolumeDescriptorSetTerminator>() == SECTOR_SIZE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primary_volume_descriptor_size() {
        assert_eq!(size_of::<PrimaryVolumeDescriptor>(), SECTOR_SIZE);
    }

    #[test]
    fn test_boot_record_size() {
        assert_eq!(size_of::<BootRecordVolumeDescriptor>(), SECTOR_SIZE);
    }

    #[test]
    fn test_terminator_size() {
        assert_eq!(size_of::<VolumeDescriptorSetTerminator>(), SECTOR_SIZE);
    }

    #[test]
    fn test_primary_volume_descriptor_fields() {
        let pvd = PrimaryVolumeDescriptor::new();
        assert_eq!(pvd.type_code, 1);
        assert_eq!(&pvd.standard_identifier, b"CD001");
        assert_eq!(pvd.version, 1);
        assert_eq!(pvd.logical_block_size.get(), 2048);
    }

    #[test]
    fn test_boot_record_fields() {
        let br = BootRecordVolumeDescriptor::new(20);
        assert_eq!(br.type_code, 0);
        assert_eq!(&br.standard_identifier, b"CD001");
        assert_eq!(br.boot_catalog_location(), 20);
        assert_eq!(
            &br.boot_system_identifier[..23],
            b"EL TORITO SPECIFICATION"
        );
    }
}
