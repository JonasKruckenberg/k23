// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! El Torito Boot Catalog structures for bootable ISO 9660 images.
//!
//! El Torito is an extension to ISO 9660 that allows booting from CD-ROM/DVD.
//! The boot catalog contains entries describing available boot images and
//! how they should be loaded.
//!
//! Boot Catalog Structure:
//! 1. Validation Entry (required, first entry)
//! 2. Initial/Default Entry (required, second entry)
//! 3. Section Header Entry (optional, for multi-platform boot)
//! 4. Section Entry (optional, additional boot images)
//!
//! Reference: "El Torito" Bootable CD-ROM Format Specification Version 1.0

extern crate alloc;

use alloc::vec::Vec;

/// Size of each boot catalog entry in bytes.
pub const BOOT_CATALOG_ENTRY_SIZE: usize = 32;

/// Platform IDs for El Torito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlatformId {
    /// 80x86 (BIOS)
    X86 = 0x00,
    /// PowerPC
    PowerPC = 0x01,
    /// Mac
    Mac = 0x02,
    /// EFI (UEFI systems)
    Efi = 0xEF,
}

impl Default for PlatformId {
    fn default() -> Self {
        Self::X86
    }
}

/// Boot media types for El Torito.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BootMediaType {
    /// No emulation - boot image is loaded directly
    NoEmulation = 0x00,
    /// 1.2 MB floppy emulation
    Floppy1_2M = 0x01,
    /// 1.44 MB floppy emulation
    Floppy1_44M = 0x02,
    /// 2.88 MB floppy emulation
    Floppy2_88M = 0x03,
    /// Hard disk emulation
    HardDisk = 0x04,
}

impl Default for BootMediaType {
    fn default() -> Self {
        Self::NoEmulation
    }
}

/// Validation Entry (first entry in boot catalog).
///
/// The validation entry verifies the integrity of the boot catalog and
/// identifies the platform. The checksum is computed such that all 16-bit
/// words sum to zero.
///
/// Reference: El Torito Specification Section 2.1
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct ValidationEntry {
    /// Header ID (must be 0x01)
    pub header_id: u8,
    /// Platform ID
    pub platform_id: u8,
    /// Reserved (0)
    pub reserved: u16,
    /// ID string (24 bytes, manufacturer/developer)
    pub id_string: [u8; 24],
    /// Checksum (sum of all 16-bit words must be 0)
    pub checksum: u16,
    /// Key byte 1 (must be 0x55)
    pub key_byte_1: u8,
    /// Key byte 2 (must be 0xAA)
    pub key_byte_2: u8,
}

impl ValidationEntry {
    /// Key bytes that must be present for a valid entry.
    pub const KEY_BYTES: (u8, u8) = (0x55, 0xAA);

    /// Creates a new validation entry for the specified platform.
    #[must_use]
    pub fn new(platform: PlatformId, id_string: &str) -> Self {
        let mut entry = Self {
            header_id: 0x01,
            platform_id: platform as u8,
            reserved: 0,
            id_string: [0; 24],
            checksum: 0,
            key_byte_1: Self::KEY_BYTES.0,
            key_byte_2: Self::KEY_BYTES.1,
        };

        // Copy ID string
        let id_bytes = id_string.as_bytes();
        let copy_len = id_bytes.len().min(24);
        entry.id_string[..copy_len].copy_from_slice(&id_bytes[..copy_len]);

        // Calculate checksum
        entry.checksum = entry.calculate_checksum();

        entry
    }

    /// Calculates the checksum for the validation entry.
    ///
    /// The checksum is the two's complement of the sum of all 16-bit words
    /// in the entry (excluding the checksum field).
    fn calculate_checksum(&self) -> u16 {
        let bytes = self.as_bytes_without_checksum();
        let mut sum: u32 = 0;

        // Sum all 16-bit words except checksum (bytes 28-29)
        for i in (0..28).step_by(2) {
            sum += u16::from_le_bytes([bytes[i], bytes[i + 1]]) as u32;
        }
        // Add key bytes
        sum += u16::from_le_bytes([self.key_byte_1, self.key_byte_2]) as u32;

        // Two's complement
        ((!sum + 1) & 0xFFFF) as u16
    }

    /// Returns the entry as bytes without the checksum field set.
    fn as_bytes_without_checksum(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes[0] = self.header_id;
        bytes[1] = self.platform_id;
        bytes[2..4].copy_from_slice(&self.reserved.to_le_bytes());
        bytes[4..28].copy_from_slice(&self.id_string);
        // bytes[28..30] = checksum (leave as 0 for calculation)
        bytes[30] = self.key_byte_1;
        bytes[31] = self.key_byte_2;
        bytes
    }

    /// Serializes the entry to a byte array.
    #[must_use]
    pub fn as_bytes(&self) -> [u8; 32] {
        let mut bytes = self.as_bytes_without_checksum();
        bytes[28..30].copy_from_slice(&self.checksum.to_le_bytes());
        bytes
    }
}

const _: () = assert!(size_of::<ValidationEntry>() == BOOT_CATALOG_ENTRY_SIZE);

/// Initial/Default Entry (second entry in boot catalog).
///
/// Describes the default boot image to be loaded. This entry is required
/// for any bootable CD-ROM.
///
/// Reference: El Torito Specification Section 2.2
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct InitialEntry {
    /// Boot indicator (0x88 = bootable, 0x00 = not bootable)
    pub boot_indicator: u8,
    /// Boot media type
    pub boot_media_type: u8,
    /// Load segment (0 = use default 0x7C0)
    pub load_segment: u16,
    /// System type (from partition table)
    pub system_type: u8,
    /// Unused (0)
    pub unused1: u8,
    /// Number of 512-byte virtual sectors to load
    pub sector_count: u16,
    /// Logical block address of the boot image
    pub load_rba: u32,
    /// Reserved/unused (20 bytes)
    pub reserved: [u8; 20],
}

impl InitialEntry {
    /// Boot indicator value for bootable entry.
    pub const BOOTABLE: u8 = 0x88;
    /// Boot indicator value for not bootable entry.
    pub const NOT_BOOTABLE: u8 = 0x00;

    /// Creates a new initial entry for a bootable image.
    ///
    /// # Arguments
    ///
    /// * `media_type` - The boot media emulation type
    /// * `load_rba` - The logical block address of the boot image
    /// * `sector_count` - Number of 512-byte sectors to load (for no emulation)
    #[must_use]
    pub fn new(media_type: BootMediaType, load_rba: u32, sector_count: u16) -> Self {
        Self {
            boot_indicator: Self::BOOTABLE,
            boot_media_type: media_type as u8,
            load_segment: 0, // Use default 0x7C0
            system_type: 0,
            unused1: 0,
            sector_count,
            load_rba: load_rba.to_le(),
            reserved: [0; 20],
        }
    }

    /// Creates a no-emulation boot entry.
    ///
    /// This is the most common type for modern bootloaders.
    ///
    /// # Arguments
    ///
    /// * `load_rba` - The logical block address of the boot image
    /// * `sector_count` - Number of 512-byte sectors to load
    #[must_use]
    pub fn no_emulation(load_rba: u32, sector_count: u16) -> Self {
        Self::new(BootMediaType::NoEmulation, load_rba, sector_count)
    }

    /// Sets the load segment for the boot image.
    ///
    /// Only meaningful for no-emulation boot.
    /// Default is 0, which means 0x7C0.
    pub fn set_load_segment(&mut self, segment: u16) {
        self.load_segment = segment.to_le();
    }

    /// Serializes the entry to a byte array.
    #[must_use]
    pub fn as_bytes(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes[0] = self.boot_indicator;
        bytes[1] = self.boot_media_type;
        bytes[2..4].copy_from_slice(&self.load_segment.to_le_bytes());
        bytes[4] = self.system_type;
        bytes[5] = self.unused1;
        bytes[6..8].copy_from_slice(&self.sector_count.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.load_rba.to_le_bytes());
        bytes[12..32].copy_from_slice(&self.reserved);
        bytes
    }
}

const _: () = assert!(size_of::<InitialEntry>() == BOOT_CATALOG_ENTRY_SIZE);

/// Section Header Entry for multi-platform boot support.
///
/// Section headers introduce additional boot entries for different platforms.
/// Each section header is followed by one or more section entries.
///
/// Reference: El Torito Specification Section 2.3
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct SectionHeaderEntry {
    /// Header indicator (0x90 = more headers follow, 0x91 = last header)
    pub header_indicator: u8,
    /// Platform ID
    pub platform_id: u8,
    /// Number of section entries following this header
    pub section_entries: u16,
    /// ID string (28 bytes)
    pub id_string: [u8; 28],
}

impl SectionHeaderEntry {
    /// Header indicator for non-final section header.
    pub const MORE_HEADERS: u8 = 0x90;
    /// Header indicator for final section header.
    pub const FINAL_HEADER: u8 = 0x91;

    /// Creates a new section header entry.
    ///
    /// # Arguments
    ///
    /// * `platform` - Platform ID for this section
    /// * `section_entries` - Number of boot entries following this header
    /// * `is_final` - Whether this is the last section header
    #[must_use]
    pub fn new(platform: PlatformId, section_entries: u16, is_final: bool) -> Self {
        Self {
            header_indicator: if is_final {
                Self::FINAL_HEADER
            } else {
                Self::MORE_HEADERS
            },
            platform_id: platform as u8,
            section_entries: section_entries.to_le(),
            id_string: [0; 28],
        }
    }

    /// Sets the ID string for this section.
    pub fn set_id_string(&mut self, id: &str) {
        let id_bytes = id.as_bytes();
        let copy_len = id_bytes.len().min(28);
        self.id_string[..copy_len].copy_from_slice(&id_bytes[..copy_len]);
    }

    /// Serializes the entry to a byte array.
    #[must_use]
    pub fn as_bytes(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes[0] = self.header_indicator;
        bytes[1] = self.platform_id;
        bytes[2..4].copy_from_slice(&self.section_entries.to_le_bytes());
        bytes[4..32].copy_from_slice(&self.id_string);
        bytes
    }
}

const _: () = assert!(size_of::<SectionHeaderEntry>() == BOOT_CATALOG_ENTRY_SIZE);

/// Section Entry for additional boot images.
///
/// Section entries follow section headers and describe additional boot
/// images for the specified platform.
///
/// Reference: El Torito Specification Section 2.4
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct SectionEntry {
    /// Boot indicator (0x88 = bootable, 0x00 = not bootable)
    pub boot_indicator: u8,
    /// Boot media type
    pub boot_media_type: u8,
    /// Load segment (0 = use default 0x7C0)
    pub load_segment: u16,
    /// System type (from partition table)
    pub system_type: u8,
    /// Unused (0)
    pub unused1: u8,
    /// Number of 512-byte virtual sectors to load
    pub sector_count: u16,
    /// Logical block address of the boot image
    pub load_rba: u32,
    /// Selection criteria type
    pub selection_criteria_type: u8,
    /// Vendor-unique selection criteria (19 bytes)
    pub selection_criteria: [u8; 19],
}

impl SectionEntry {
    /// Creates a new section entry for a bootable image.
    #[must_use]
    pub fn new(media_type: BootMediaType, load_rba: u32, sector_count: u16) -> Self {
        Self {
            boot_indicator: InitialEntry::BOOTABLE,
            boot_media_type: media_type as u8,
            load_segment: 0,
            system_type: 0,
            unused1: 0,
            sector_count: sector_count.to_le(),
            load_rba: load_rba.to_le(),
            selection_criteria_type: 0,
            selection_criteria: [0; 19],
        }
    }

    /// Creates a no-emulation boot section entry.
    #[must_use]
    pub fn no_emulation(load_rba: u32, sector_count: u16) -> Self {
        Self::new(BootMediaType::NoEmulation, load_rba, sector_count)
    }

    /// Serializes the entry to a byte array.
    #[must_use]
    pub fn as_bytes(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes[0] = self.boot_indicator;
        bytes[1] = self.boot_media_type;
        bytes[2..4].copy_from_slice(&self.load_segment.to_le_bytes());
        bytes[4] = self.system_type;
        bytes[5] = self.unused1;
        bytes[6..8].copy_from_slice(&self.sector_count.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.load_rba.to_le_bytes());
        bytes[12] = self.selection_criteria_type;
        bytes[13..32].copy_from_slice(&self.selection_criteria);
        bytes
    }
}

const _: () = assert!(size_of::<SectionEntry>() == BOOT_CATALOG_ENTRY_SIZE);

/// Boot Catalog builder.
///
/// Builds a complete boot catalog with validation entry, initial/default
/// entry, and optional section headers and entries for multi-platform boot.
#[derive(Default)]
pub struct BootCatalogBuilder {
    /// Platform for the default boot entry
    platform: PlatformId,
    /// ID string for the validation entry
    id_string: [u8; 24],
    /// Default boot entry
    default_entry: Option<InitialEntry>,
    /// Additional sections for multi-platform boot
    sections: Vec<(SectionHeaderEntry, Vec<SectionEntry>)>,
}

impl BootCatalogBuilder {
    /// Creates a new boot catalog builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the platform ID for the default boot entry.
    pub fn platform(&mut self, platform: PlatformId) -> &mut Self {
        self.platform = platform;
        self
    }

    /// Sets the ID string for the validation entry.
    pub fn id_string(&mut self, id: &str) -> &mut Self {
        let id_bytes = id.as_bytes();
        let copy_len = id_bytes.len().min(24);
        self.id_string = [0; 24];
        self.id_string[..copy_len].copy_from_slice(&id_bytes[..copy_len]);
        self
    }

    /// Sets the default boot entry.
    ///
    /// # Arguments
    ///
    /// * `media_type` - Boot media emulation type
    /// * `load_rba` - Logical block address of the boot image
    /// * `sector_count` - Number of 512-byte sectors to load
    pub fn default_boot_entry(
        &mut self,
        media_type: BootMediaType,
        load_rba: u32,
        sector_count: u16,
    ) -> &mut Self {
        self.default_entry = Some(InitialEntry::new(media_type, load_rba, sector_count));
        self
    }

    /// Adds a section for multi-platform boot.
    ///
    /// # Arguments
    ///
    /// * `platform` - Platform ID for this section
    /// * `entries` - Boot entries for this platform
    pub fn add_section(&mut self, platform: PlatformId, entries: Vec<SectionEntry>) -> &mut Self {
        let header = SectionHeaderEntry::new(platform, entries.len() as u16, false);
        self.sections.push((header, entries));
        self
    }

    /// Returns the total size of the boot catalog in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        let mut size = BOOT_CATALOG_ENTRY_SIZE * 2; // Validation + Initial entries

        for (_, entries) in &self.sections {
            size += BOOT_CATALOG_ENTRY_SIZE; // Section header
            size += entries.len() * BOOT_CATALOG_ENTRY_SIZE; // Section entries
        }

        size
    }

    /// Returns the number of sectors required for the boot catalog.
    #[must_use]
    pub fn sectors(&self) -> u32 {
        let size = self.size();
        ((size + crate::types::SECTOR_SIZE - 1) / crate::types::SECTOR_SIZE) as u32
    }

    /// Builds the boot catalog.
    ///
    /// Returns the catalog as a vector of bytes.
    ///
    /// # Panics
    ///
    /// Panics if no default boot entry has been set.
    #[must_use]
    pub fn build(&mut self) -> Vec<u8> {
        let default_entry = self
            .default_entry
            .take()
            .expect("default boot entry must be set");

        // Create validation entry
        let id_str = core::str::from_utf8(&self.id_string)
            .unwrap_or("")
            .trim_end_matches('\0');
        let validation = ValidationEntry::new(self.platform, id_str);

        let mut catalog = Vec::with_capacity(self.size());

        // Write validation entry
        catalog.extend_from_slice(&validation.as_bytes());

        // Write initial/default entry
        catalog.extend_from_slice(&default_entry.as_bytes());

        // Write sections
        let num_sections = self.sections.len();
        for (i, (mut header, entries)) in self.sections.drain(..).enumerate() {
            // Mark the last section header
            if i == num_sections - 1 {
                header.header_indicator = SectionHeaderEntry::FINAL_HEADER;
            }
            catalog.extend_from_slice(&header.as_bytes());

            for entry in entries {
                catalog.extend_from_slice(&entry.as_bytes());
            }
        }

        // Pad to sector boundary
        let padding = crate::types::SECTOR_SIZE - (catalog.len() % crate::types::SECTOR_SIZE);
        if padding < crate::types::SECTOR_SIZE {
            catalog.resize(catalog.len() + padding, 0);
        }

        catalog
    }
}

/// Boot image descriptor.
///
/// Represents a boot image to be included in the ISO.
#[derive(Clone)]
pub struct BootImage {
    /// Platform this boot image is for
    pub platform: PlatformId,
    /// Boot media emulation type
    pub media_type: BootMediaType,
    /// Boot image data
    pub data: Vec<u8>,
    /// Load segment (0 = default 0x7C0)
    pub load_segment: u16,
}

impl BootImage {
    /// Creates a new boot image.
    #[must_use]
    pub fn new(platform: PlatformId, media_type: BootMediaType, data: Vec<u8>) -> Self {
        Self {
            platform,
            media_type,
            data,
            load_segment: 0,
        }
    }

    /// Creates a no-emulation boot image for x86 BIOS.
    #[must_use]
    pub fn bios_no_emulation(data: Vec<u8>) -> Self {
        Self::new(PlatformId::X86, BootMediaType::NoEmulation, data)
    }

    /// Creates a no-emulation boot image for EFI.
    #[must_use]
    pub fn efi_no_emulation(data: Vec<u8>) -> Self {
        Self::new(PlatformId::Efi, BootMediaType::NoEmulation, data)
    }

    /// Returns the number of 512-byte virtual sectors for this image.
    #[must_use]
    pub fn sector_count(&self) -> u16 {
        let sectors = (self.data.len() + 511) / 512;
        sectors.min(0xFFFF) as u16
    }

    /// Returns the number of 2048-byte ISO sectors for this image.
    #[must_use]
    pub fn iso_sector_count(&self) -> u32 {
        ((self.data.len() + crate::types::SECTOR_SIZE - 1) / crate::types::SECTOR_SIZE) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_entry_size() {
        assert_eq!(size_of::<ValidationEntry>(), BOOT_CATALOG_ENTRY_SIZE);
    }

    #[test]
    fn test_initial_entry_size() {
        assert_eq!(size_of::<InitialEntry>(), BOOT_CATALOG_ENTRY_SIZE);
    }

    #[test]
    fn test_section_header_size() {
        assert_eq!(size_of::<SectionHeaderEntry>(), BOOT_CATALOG_ENTRY_SIZE);
    }

    #[test]
    fn test_section_entry_size() {
        assert_eq!(size_of::<SectionEntry>(), BOOT_CATALOG_ENTRY_SIZE);
    }

    #[test]
    fn test_validation_entry_checksum() {
        let entry = ValidationEntry::new(PlatformId::X86, "TEST");
        let bytes = entry.as_bytes();

        // Verify checksum - sum of all 16-bit words should be 0
        let mut sum: u32 = 0;
        for i in (0..32).step_by(2) {
            sum += u16::from_le_bytes([bytes[i], bytes[i + 1]]) as u32;
        }
        assert_eq!(sum & 0xFFFF, 0, "checksum validation failed");
    }

    #[test]
    fn test_validation_entry_key_bytes() {
        let entry = ValidationEntry::new(PlatformId::X86, "TEST");
        let bytes = entry.as_bytes();
        assert_eq!(bytes[30], 0x55);
        assert_eq!(bytes[31], 0xAA);
    }

    #[test]
    fn test_initial_entry() {
        let entry = InitialEntry::no_emulation(100, 4);
        let bytes = entry.as_bytes();
        assert_eq!(bytes[0], 0x88); // bootable
        assert_eq!(bytes[1], 0x00); // no emulation
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), 4); // sector count
        assert_eq!(
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            100
        ); // load_rba
    }

    #[test]
    fn test_boot_catalog_builder() {
        let mut builder = BootCatalogBuilder::new();
        builder
            .platform(PlatformId::X86)
            .id_string("TEST ISO")
            .default_boot_entry(BootMediaType::NoEmulation, 100, 4);

        let catalog = builder.build();
        assert!(!catalog.is_empty());
        assert_eq!(catalog.len() % crate::types::SECTOR_SIZE, 0);
    }

    #[test]
    fn test_boot_image() {
        let data = vec![0u8; 4096]; // 8 virtual sectors
        let image = BootImage::bios_no_emulation(data);
        assert_eq!(image.sector_count(), 8);
        assert_eq!(image.iso_sector_count(), 2);
    }
}
