// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! ISO 9660 (ECMA-119) filesystem image creation library with El Torito boot support.
//!
//! This library provides functionality to create ISO 9660 filesystem images,
//! commonly used for CD-ROMs, DVDs, and bootable disk images. It supports:
//!
//! - **ECMA-119 compliant filesystem structure**: System area, volume descriptors,
//!   path tables, and directory records following the ISO 9660 standard.
//!
//! - **El Torito boot specification**: Create bootable ISO images for BIOS and
//!   UEFI systems with support for multiple boot images.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use iso9660::{IsoBuilder, BootImage};
//!
//! // Create a simple ISO image
//! let mut builder = IsoBuilder::new("MY_VOLUME");
//! builder.add_file("README.TXT", b"Hello, World!")?;
//!
//! let iso_data = builder.build()?;
//! ```
//!
//! # Creating a Bootable ISO
//!
//! ```rust,ignore
//! use iso9660::{IsoBuilder, BootImage};
//!
//! let mut builder = IsoBuilder::new("BOOTABLE");
//!
//! // Add your bootloader
//! let bootloader = std::fs::read("bootloader.bin")?;
//! builder.set_boot_image(BootImage::bios_no_emulation(bootloader));
//!
//! // Optionally add EFI boot support
//! let efi_bootloader = std::fs::read("efi_boot.efi")?;
//! builder.add_boot_image(BootImage::efi_no_emulation(efi_bootloader));
//!
//! // Add files
//! builder.add_file("KERNEL.BIN", &kernel_data)?;
//!
//! let iso_data = builder.build()?;
//! ```
//!
//! # ISO 9660 Structure
//!
//! An ISO 9660 filesystem consists of:
//!
//! 1. **System Area** (sectors 0-15): 32KB reserved for system use
//! 2. **Volume Descriptors** (starting at sector 16):
//!    - Primary Volume Descriptor (required)
//!    - Boot Record (for El Torito bootable ISOs)
//!    - Volume Descriptor Set Terminator (required)
//! 3. **Path Tables**: Index of directories for fast lookup
//! 4. **Directory Records**: File and directory metadata
//! 5. **File Data**: Actual file contents
//!
//! # El Torito Boot Support
//!
//! El Torito is the standard for bootable CD-ROMs. This library supports:
//!
//! - **No Emulation mode**: Direct boot of a loader image (most common)
//! - **Floppy Emulation**: Emulate a 1.2MB, 1.44MB, or 2.88MB floppy
//! - **Hard Disk Emulation**: Emulate a hard disk
//!
//! The boot catalog supports multiple platforms:
//! - x86 BIOS (Platform ID 0x00)
//! - EFI/UEFI (Platform ID 0xEF)
//! - PowerPC (Platform ID 0x01)
//! - Mac (Platform ID 0x02)
//!
//! # References
//!
//! - [ECMA-119](https://www.ecma-international.org/publications-and-standards/standards/ecma-119/):
//!   Volume and File Structure of CDROM for Information Interchange
//! - [El Torito Specification](https://pdos.csail.mit.edu/6.828/2014/readings/boot-cdrom.pdf):
//!   Bootable CD-ROM Format Specification Version 1.0

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod builder;
mod directory;
mod eltorito;
mod error;
mod path_table;
mod types;
mod volume;

// Re-export main builder API
pub use builder::IsoBuilder;

// Re-export El Torito types
pub use eltorito::{
    BootCatalogBuilder, BootImage, BootMediaType, InitialEntry, PlatformId, SectionEntry,
    SectionHeaderEntry, ValidationEntry,
};

// Re-export error types
pub use error::{Error, Result};

// Re-export volume descriptor types
pub use volume::{
    BootRecordVolumeDescriptor, PrimaryVolumeDescriptor, VolumeDescriptorSetTerminator,
    VolumeDescriptorType, STANDARD_IDENTIFIER,
};

// Re-export directory types
pub use directory::{
    flags as file_flags, iso9660_name_length, to_iso9660_name, DirectoryRecord,
    DirectoryRecordBuilder, DIRECTORY_RECORD_MIN_SIZE, MAX_DIRECTORY_IDENTIFIER_LENGTH,
    MAX_FILE_IDENTIFIER_LENGTH,
};

// Re-export path table types
pub use path_table::{PathTableBuilder, PathTableRecord};

// Re-export primitive types
pub use types::{
    BothEndian16, BothEndian32, DirectoryDateTime, StrA, VolumeDateTime, SECTOR_SIZE,
    SYSTEM_AREA_SECTORS, SYSTEM_AREA_SIZE,
};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_create_minimal_iso() {
        let mut builder = IsoBuilder::new("TEST");
        let iso = builder.build().unwrap();

        // Minimum ISO size: system area (16 sectors) + PVD + terminator + path tables + root dir
        assert!(iso.len() >= 20 * SECTOR_SIZE);
    }

    #[test]
    fn test_create_iso_with_files() {
        let mut builder = IsoBuilder::new("FILES");
        builder.add_file("README.TXT", b"Test content").unwrap();
        builder.add_file("DATA/INFO.DAT", b"Nested file").unwrap();

        let iso = builder.build().unwrap();
        assert!(!iso.is_empty());
    }

    #[test]
    fn test_create_bootable_iso() {
        let mut builder = IsoBuilder::new("BOOTABLE");

        // Minimal bootloader (infinite loop)
        let boot_code = vec![0xEB, 0xFE, 0x00, 0x00];
        builder.set_boot_image(BootImage::bios_no_emulation(boot_code));

        let iso = builder.build().unwrap();

        // Verify boot record exists at sector 17
        let br_offset = 17 * SECTOR_SIZE;
        assert_eq!(iso[br_offset], 0); // Boot record type
        assert_eq!(&iso[br_offset + 1..br_offset + 6], b"CD001");
    }

    #[test]
    fn test_create_multi_boot_iso() {
        let mut builder = IsoBuilder::new("MULTIBOOT");

        // BIOS boot image
        let bios_boot = vec![0xEB, 0xFE];
        builder.set_boot_image(BootImage::bios_no_emulation(bios_boot));

        // EFI boot image
        let efi_boot = vec![0x00; 512]; // Placeholder EFI image
        builder.add_boot_image(BootImage::efi_no_emulation(efi_boot));

        let iso = builder.build().unwrap();
        assert!(!iso.is_empty());
    }

    #[test]
    fn test_volume_identifiers() {
        let mut builder = IsoBuilder::new("VOLID");
        builder
            .system_id("LINUX")
            .publisher_id("PUBLISHER")
            .application_id("MY_APP");

        let iso = builder.build().unwrap();

        // Check PVD contains our identifiers
        let pvd_offset = 16 * SECTOR_SIZE;

        // System identifier at offset 8, 32 bytes
        let sys_id = &iso[pvd_offset + 8..pvd_offset + 40];
        assert!(sys_id.starts_with(b"LINUX"));

        // Volume identifier at offset 40, 32 bytes
        let vol_id = &iso[pvd_offset + 40..pvd_offset + 72];
        assert!(vol_id.starts_with(b"VOLID"));
    }

    #[test]
    fn test_both_endian_types() {
        let val16 = BothEndian16::new(0x1234);
        assert_eq!(val16.get(), 0x1234);

        let val32 = BothEndian32::new(0x12345678);
        assert_eq!(val32.get(), 0x12345678);
    }

    #[test]
    fn test_datetime_types() {
        let dir_dt = DirectoryDateTime::new(2025, 6, 15, 12, 30, 45, 0);
        assert_eq!(dir_dt.year(), 2025);

        let vol_dt = VolumeDateTime::new(2025, 6, 15, 12, 30, 45, 0, 0);
        assert_eq!(&vol_dt.year, b"2025");
    }
}
