// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! ISO 9660 image builder.
//!
//! This module provides a builder API for creating ISO 9660 filesystem images
//! with optional El Torito boot support.
//!
//! # Example
//!
//! ```rust,ignore
//! use iso9660::{IsoBuilder, BootImage, PlatformId, BootMediaType};
//!
//! let mut builder = IsoBuilder::new("MY_VOLUME");
//!
//! // Add files
//! builder.add_file("README.TXT", b"Hello, World!");
//! builder.add_file("DATA/FILE.DAT", &file_data);
//!
//! // Add boot image for BIOS boot
//! let boot_image = BootImage::bios_no_emulation(bootloader_data);
//! builder.set_boot_image(boot_image);
//!
//! // Build the ISO
//! let iso_data = builder.build()?;
//! ```

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::directory::{
    iso9660_name_length, to_iso9660_name, DirectoryRecordBuilder, MAX_FILE_IDENTIFIER_LENGTH,
};
use crate::eltorito::BootCatalogBuilder;
pub use crate::eltorito::BootImage;
use crate::error::{Error, Result};
use crate::path_table::PathTableBuilder;
use crate::types::{VolumeDateTime, SECTOR_SIZE, SYSTEM_AREA_SECTORS};
use crate::volume::{
    BootRecordVolumeDescriptor, PrimaryVolumeDescriptor, VolumeDescriptorSetTerminator,
};

/// A file entry to be included in the ISO.
#[derive(Clone)]
struct FileEntry {
    /// ISO 9660 filename (uppercase, 8.3 format with version)
    iso_name: [u8; 32],
    /// File data
    data: Vec<u8>,
    /// Assigned sector location (set during build)
    location: u32,
}

/// A directory entry in the ISO filesystem.
#[derive(Clone, Default)]
struct DirectoryEntry {
    /// ISO 9660 directory name (uppercase)
    iso_name: [u8; 32],
    /// Child directories
    children: BTreeMap<String, DirectoryEntry>,
    /// Files in this directory
    files: BTreeMap<String, FileEntry>,
    /// Assigned sector location (set during build)
    location: u32,
    /// Directory size in bytes (set during build)
    size: u32,
    /// Path table directory number (set during build)
    dir_number: u16,
}

/// Builder for creating ISO 9660 filesystem images.
///
/// This builder supports:
/// - Adding files and directories
/// - El Torito boot support (BIOS and EFI)
/// - Volume metadata (identifiers, dates)
pub struct IsoBuilder {
    /// Volume identifier (up to 32 characters)
    volume_id: String,
    /// System identifier
    system_id: String,
    /// Publisher identifier
    publisher_id: String,
    /// Application identifier
    application_id: String,
    /// Root directory
    root: DirectoryEntry,
    /// Primary boot image (for El Torito)
    boot_image: Option<BootImage>,
    /// Additional boot images for multi-platform boot
    additional_boot_images: Vec<BootImage>,
    /// Volume creation date
    creation_date: Option<VolumeDateTime>,
}

impl IsoBuilder {
    /// Creates a new ISO builder with the specified volume identifier.
    ///
    /// # Arguments
    ///
    /// * `volume_id` - Volume identifier (up to 32 characters, will be uppercased)
    #[must_use]
    pub fn new(volume_id: &str) -> Self {
        Self {
            volume_id: volume_id.to_ascii_uppercase(),
            system_id: String::new(),
            publisher_id: String::new(),
            application_id: String::from("ISO9660-RS"),
            root: DirectoryEntry::default(),
            boot_image: None,
            additional_boot_images: Vec::new(),
            creation_date: None,
        }
    }

    /// Sets the system identifier.
    pub fn system_id(&mut self, id: &str) -> &mut Self {
        self.system_id = id.to_string();
        self
    }

    /// Sets the publisher identifier.
    pub fn publisher_id(&mut self, id: &str) -> &mut Self {
        self.publisher_id = id.to_string();
        self
    }

    /// Sets the application identifier.
    pub fn application_id(&mut self, id: &str) -> &mut Self {
        self.application_id = id.to_string();
        self
    }

    /// Sets the volume creation date.
    pub fn creation_date(&mut self, date: VolumeDateTime) -> &mut Self {
        self.creation_date = Some(date);
        self
    }

    /// Sets the primary boot image for El Torito boot.
    ///
    /// This enables bootable ISO creation. The boot image will be stored
    /// in the ISO and referenced by the El Torito boot catalog.
    pub fn set_boot_image(&mut self, image: BootImage) -> &mut Self {
        self.boot_image = Some(image);
        self
    }

    /// Adds an additional boot image for multi-platform boot.
    ///
    /// Use this to add EFI boot support alongside BIOS boot.
    pub fn add_boot_image(&mut self, image: BootImage) -> &mut Self {
        self.additional_boot_images.push(image);
        self
    }

    /// Adds a file to the ISO image.
    ///
    /// The path should use forward slashes as separators. Parent directories
    /// will be created automatically.
    ///
    /// # Arguments
    ///
    /// * `path` - Path within the ISO (e.g., "DIR/FILE.TXT")
    /// * `data` - File contents
    ///
    /// # Errors
    ///
    /// Returns an error if the filename is too long or contains invalid characters.
    pub fn add_file(&mut self, path: &str, data: &[u8]) -> Result<&mut Self> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return Err(Error::InvalidIdentifier("empty path"));
        }

        // Navigate to the parent directory, creating intermediate directories
        let mut current_dir = &mut self.root;
        for &dir_name in &parts[..parts.len() - 1] {
            if dir_name.len() > MAX_FILE_IDENTIFIER_LENGTH {
                return Err(Error::IdentifierTooLong {
                    identifier: "directory name",
                    max_length: MAX_FILE_IDENTIFIER_LENGTH,
                });
            }

            let dir_key = dir_name.to_ascii_uppercase();
            current_dir = current_dir
                .children
                .entry(dir_key.clone())
                .or_insert_with(|| {
                    let mut entry = DirectoryEntry::default();
                    entry.iso_name = to_iso9660_name(dir_name, true);
                    entry
                });
        }

        // Add the file
        let file_name = parts.last().unwrap();
        if file_name.len() > MAX_FILE_IDENTIFIER_LENGTH {
            return Err(Error::IdentifierTooLong {
                identifier: "file name",
                max_length: MAX_FILE_IDENTIFIER_LENGTH,
            });
        }

        let file_key = file_name.to_ascii_uppercase();
        current_dir.files.insert(
            file_key,
            FileEntry {
                iso_name: to_iso9660_name(file_name, false),
                data: data.to_vec(),
                location: 0,
            },
        );

        Ok(self)
    }

    /// Adds a directory to the ISO image.
    ///
    /// Parent directories will be created automatically.
    ///
    /// # Arguments
    ///
    /// * `path` - Directory path within the ISO
    ///
    /// # Errors
    ///
    /// Returns an error if the directory name is too long.
    pub fn add_directory(&mut self, path: &str) -> Result<&mut Self> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if parts.is_empty() {
            return Ok(self);
        }

        let mut current_dir = &mut self.root;
        for &dir_name in &parts {
            if dir_name.len() > MAX_FILE_IDENTIFIER_LENGTH {
                return Err(Error::IdentifierTooLong {
                    identifier: "directory name",
                    max_length: MAX_FILE_IDENTIFIER_LENGTH,
                });
            }

            let dir_key = dir_name.to_ascii_uppercase();
            current_dir = current_dir
                .children
                .entry(dir_key.clone())
                .or_insert_with(|| {
                    let mut entry = DirectoryEntry::default();
                    entry.iso_name = to_iso9660_name(dir_name, true);
                    entry
                });
        }

        Ok(self)
    }

    /// Builds the ISO 9660 image.
    ///
    /// Returns the complete ISO image as a byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if the image cannot be built.
    pub fn build(&mut self) -> Result<Vec<u8>> {
        // Calculate layout
        // Sectors 0-15: System Area (32 KB)
        // Sector 16: Primary Volume Descriptor
        // Sector 17: Boot Record (if bootable) or Terminator
        // Sector 18: Terminator (if bootable)
        // Following sectors: Path tables, directories, boot catalog, boot images, files

        let has_boot = self.boot_image.is_some();
        let mut current_sector = SYSTEM_AREA_SECTORS;

        // Volume descriptors
        let pvd_sector = current_sector;
        current_sector += 1;

        let boot_record_sector = if has_boot {
            let sector = current_sector;
            current_sector += 1;
            Some(sector)
        } else {
            None
        };

        let terminator_sector = current_sector;
        current_sector += 1;

        // Path tables (L and M)
        let path_table_l_sector = current_sector;
        current_sector += 1; // Reserve at least one sector for L path table

        let path_table_m_sector = current_sector;
        current_sector += 1; // Reserve at least one sector for M path table

        // Root directory
        let root_directory_sector = current_sector;
        self.root.location = root_directory_sector;

        // Build path table and assign directory locations
        let mut path_table = PathTableBuilder::new();
        self.root.dir_number = path_table.add_root(root_directory_sector);

        // Assign directory locations (breadth-first)
        self.assign_directory_locations(&mut current_sector, &mut path_table)?;

        // Boot catalog location (if bootable)
        let boot_catalog_sector = if has_boot {
            let sector = current_sector;
            current_sector += 1;
            Some(sector)
        } else {
            None
        };

        // Boot image location (if bootable)
        let boot_image_sector = if let Some(ref image) = self.boot_image {
            let sector = current_sector;
            current_sector += image.iso_sector_count();
            Some(sector)
        } else {
            None
        };

        // Additional boot images
        let mut additional_boot_sectors = Vec::new();
        for image in &self.additional_boot_images {
            additional_boot_sectors.push(current_sector);
            current_sector += image.iso_sector_count();
        }

        // Assign file locations
        self.assign_file_locations(&mut current_sector)?;

        // Total volume size
        let volume_size = current_sector;

        // Path table size
        let path_table_size = path_table.size() as u32;

        // Now build the actual ISO image
        let mut iso = vec![0u8; volume_size as usize * SECTOR_SIZE];

        // Write Primary Volume Descriptor
        let mut pvd = PrimaryVolumeDescriptor::new();
        pvd.set_volume_identifier(&self.volume_id);
        if !self.system_id.is_empty() {
            pvd.set_system_identifier(&self.system_id);
        }
        if !self.publisher_id.is_empty() {
            pvd.set_publisher_identifier(&self.publisher_id);
        }
        pvd.set_application_identifier(&self.application_id);
        pvd.set_volume_space_size(volume_size);
        pvd.set_path_table(path_table_size, path_table_l_sector, path_table_m_sector);

        // Set root directory info
        let root_size = self.calculate_directory_size(&self.root);
        pvd.set_root_directory(root_directory_sector, root_size);

        if let Some(date) = &self.creation_date {
            pvd.set_creation_date(*date);
        }

        self.write_sector(&mut iso, pvd_sector, pvd.as_bytes());

        // Write Boot Record (if bootable)
        if let Some(sector) = boot_record_sector {
            let boot_record =
                BootRecordVolumeDescriptor::new(boot_catalog_sector.unwrap());
            self.write_sector(&mut iso, sector, boot_record.as_bytes());
        }

        // Write Volume Descriptor Set Terminator
        let terminator = VolumeDescriptorSetTerminator::new();
        self.write_sector(&mut iso, terminator_sector, terminator.as_bytes());

        // Write path tables
        let mut path_table_buf = vec![0u8; SECTOR_SIZE];
        path_table.write_le(&mut path_table_buf);
        self.write_sector(&mut iso, path_table_l_sector, &path_table_buf);

        path_table_buf.fill(0);
        path_table.write_be(&mut path_table_buf);
        self.write_sector(&mut iso, path_table_m_sector, &path_table_buf);

        // Write directories
        self.write_directories(&mut iso, &self.root.clone(), root_directory_sector)?;

        // Write boot catalog (if bootable)
        if let (Some(catalog_sector), Some(image_sector)) =
            (boot_catalog_sector, boot_image_sector)
        {
            let boot_image = self.boot_image.as_ref().unwrap();
            let mut catalog_builder = BootCatalogBuilder::new();
            catalog_builder
                .platform(boot_image.platform)
                .id_string(&self.volume_id)
                .default_boot_entry(
                    boot_image.media_type,
                    image_sector,
                    boot_image.sector_count(),
                );

            // Add additional boot images as sections
            for (i, image) in self.additional_boot_images.iter().enumerate() {
                let entry = crate::eltorito::SectionEntry::new(
                    image.media_type,
                    additional_boot_sectors[i],
                    image.sector_count(),
                );
                catalog_builder.add_section(image.platform, alloc::vec![entry]);
            }

            let catalog = catalog_builder.build();
            self.write_data(&mut iso, catalog_sector, &catalog);

            // Write boot images
            self.write_data(&mut iso, image_sector, &boot_image.data);
        }

        // Write additional boot images
        for (i, image) in self.additional_boot_images.iter().enumerate() {
            self.write_data(&mut iso, additional_boot_sectors[i], &image.data);
        }

        // Write file data
        self.write_files(&mut iso)?;

        Ok(iso)
    }

    /// Assigns sector locations to all directories.
    fn assign_directory_locations(
        &mut self,
        current_sector: &mut u32,
        path_table: &mut PathTableBuilder,
    ) -> Result<()> {
        // Process directories level by level (breadth-first)
        let mut to_process: Vec<(String, u16)> = Vec::new();

        // First, assign location to root and collect its children
        let root_size = self.calculate_directory_size(&self.root);
        self.root.size = root_size;
        *current_sector += sectors_for_size(root_size);

        for (name, child) in &mut self.root.children {
            child.location = *current_sector;
            let child_size = calculate_directory_size(child);
            child.size = child_size;
            *current_sector += sectors_for_size(child_size);

            child.dir_number = path_table.add_directory(
                core::str::from_utf8(&child.iso_name)
                    .unwrap_or("")
                    .trim_end_matches('\0'),
                child.location,
                self.root.dir_number,
            );
            to_process.push((name.clone(), child.dir_number));
        }

        // Process remaining directories
        while !to_process.is_empty() {
            let mut next_level = Vec::new();

            for (dir_path, parent_num) in to_process {
                let dir = self.find_directory_mut(&dir_path);
                if let Some(dir) = dir {
                    for (name, child) in &mut dir.children {
                        child.location = *current_sector;
                        let child_size = calculate_directory_size(child);
                        child.size = child_size;
                        *current_sector += sectors_for_size(child_size);

                        child.dir_number = path_table.add_directory(
                            core::str::from_utf8(&child.iso_name)
                                .unwrap_or("")
                                .trim_end_matches('\0'),
                            child.location,
                            parent_num,
                        );
                        next_level.push((format!("{dir_path}/{name}"), child.dir_number));
                    }
                }
            }

            to_process = next_level;
        }

        Ok(())
    }

    /// Finds a directory by path.
    fn find_directory_mut(&mut self, path: &str) -> Option<&mut DirectoryEntry> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = &mut self.root;

        for part in parts {
            current = current.children.get_mut(part)?;
        }

        Some(current)
    }

    /// Calculates the size of a directory entry table.
    fn calculate_directory_size(&self, dir: &DirectoryEntry) -> u32 {
        calculate_directory_size(dir)
    }

    /// Assigns sector locations to all files.
    fn assign_file_locations(&mut self, current_sector: &mut u32) -> Result<()> {
        self.assign_file_locations_recursive(&mut self.root.clone(), current_sector)?;
        self.apply_file_locations(&self.root.clone())?;
        Ok(())
    }

    fn assign_file_locations_recursive(
        &self,
        dir: &mut DirectoryEntry,
        current_sector: &mut u32,
    ) -> Result<()> {
        for file in dir.files.values_mut() {
            file.location = *current_sector;
            let file_sectors = sectors_for_size(file.data.len() as u32);
            *current_sector += file_sectors;
        }

        for child in dir.children.values_mut() {
            self.assign_file_locations_recursive(child, current_sector)?;
        }

        Ok(())
    }

    fn apply_file_locations(&mut self, template: &DirectoryEntry) -> Result<()> {
        // Apply file locations from template to actual root
        apply_file_locations_recursive(&mut self.root, template);
        Ok(())
    }

    /// Writes a sector to the ISO image.
    fn write_sector(&self, iso: &mut [u8], sector: u32, data: &[u8]) {
        let offset = sector as usize * SECTOR_SIZE;
        let len = data.len().min(SECTOR_SIZE);
        iso[offset..offset + len].copy_from_slice(&data[..len]);
    }

    /// Writes data spanning multiple sectors.
    fn write_data(&self, iso: &mut [u8], start_sector: u32, data: &[u8]) {
        let offset = start_sector as usize * SECTOR_SIZE;
        iso[offset..offset + data.len()].copy_from_slice(data);
    }

    /// Writes all directory entries.
    fn write_directories(
        &self,
        iso: &mut [u8],
        dir: &DirectoryEntry,
        parent_location: u32,
    ) -> Result<()> {
        let offset = dir.location as usize * SECTOR_SIZE;
        let mut pos = 0;

        // "." entry
        let self_entry = DirectoryRecordBuilder::new_self_entry(dir.location, dir.size);
        pos += self_entry.write_to(&mut iso[offset + pos..]);

        // ".." entry
        let parent_entry = DirectoryRecordBuilder::new_parent_entry(
            parent_location,
            SECTOR_SIZE as u32, // Parent size (simplified)
        );
        pos += parent_entry.write_to(&mut iso[offset + pos..]);

        // Child directories
        for child in dir.children.values() {
            let name = core::str::from_utf8(&child.iso_name)
                .unwrap_or("")
                .trim_end_matches('\0');
            let entry = DirectoryRecordBuilder::new_directory(name, child.location, child.size);
            pos += entry.write_to(&mut iso[offset + pos..]);
        }

        // Files
        for file in dir.files.values() {
            let name = core::str::from_utf8(&file.iso_name)
                .unwrap_or("")
                .trim_end_matches('\0');
            let entry =
                DirectoryRecordBuilder::new_file(name, file.location, file.data.len() as u32);
            pos += entry.write_to(&mut iso[offset + pos..]);
        }

        // Recursively write child directories
        for child in dir.children.values() {
            self.write_directories(iso, child, dir.location)?;
        }

        Ok(())
    }

    /// Writes all file data.
    fn write_files(&self, iso: &mut [u8]) -> Result<()> {
        self.write_files_recursive(&self.root, iso)
    }

    fn write_files_recursive(&self, dir: &DirectoryEntry, iso: &mut [u8]) -> Result<()> {
        for file in dir.files.values() {
            self.write_data(iso, file.location, &file.data);
        }

        for child in dir.children.values() {
            self.write_files_recursive(child, iso)?;
        }

        Ok(())
    }
}

/// Calculates the number of sectors needed for the given size.
fn sectors_for_size(size: u32) -> u32 {
    (size + SECTOR_SIZE as u32 - 1) / SECTOR_SIZE as u32
}

/// Calculates the size of a directory entry table.
fn calculate_directory_size(dir: &DirectoryEntry) -> u32 {
    let mut size = 0u32;

    // "." entry (34 bytes)
    size += 34;

    // ".." entry (34 bytes)
    size += 34;

    // Child directories
    for child in dir.children.values() {
        let name_len = iso9660_name_length(&child.iso_name) as u32;
        let record_size = 33 + name_len;
        // Pad to even length
        size += if record_size % 2 == 0 {
            record_size
        } else {
            record_size + 1
        };
    }

    // Files
    for file in dir.files.values() {
        let name_len = iso9660_name_length(&file.iso_name) as u32;
        let record_size = 33 + name_len;
        // Pad to even length
        size += if record_size % 2 == 0 {
            record_size
        } else {
            record_size + 1
        };
    }

    size
}

/// Recursively applies file locations from a template to the target directory.
fn apply_file_locations_recursive(target: &mut DirectoryEntry, source: &DirectoryEntry) {
    for (name, source_file) in &source.files {
        if let Some(target_file) = target.files.get_mut(name) {
            target_file.location = source_file.location;
        }
    }

    for (name, source_child) in &source.children {
        if let Some(target_child) = target.children.get_mut(name) {
            apply_file_locations_recursive(target_child, source_child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iso_builder_new() {
        let builder = IsoBuilder::new("TEST_VOLUME");
        assert_eq!(builder.volume_id, "TEST_VOLUME");
    }

    #[test]
    fn test_add_file() {
        let mut builder = IsoBuilder::new("TEST");
        builder.add_file("README.TXT", b"Hello").unwrap();
        assert!(!builder.root.files.is_empty());
    }

    #[test]
    fn test_add_nested_file() {
        let mut builder = IsoBuilder::new("TEST");
        builder.add_file("DIR/SUBDIR/FILE.TXT", b"Nested").unwrap();
        assert!(!builder.root.children.is_empty());
    }

    #[test]
    fn test_build_minimal_iso() {
        let mut builder = IsoBuilder::new("MINIMAL");
        let iso = builder.build().unwrap();

        // Check minimum size (system area + PVD + terminator + path tables + root dir)
        assert!(iso.len() >= 20 * SECTOR_SIZE);

        // Check PVD magic
        let pvd_offset = 16 * SECTOR_SIZE;
        assert_eq!(iso[pvd_offset], 1); // Type code
        assert_eq!(&iso[pvd_offset + 1..pvd_offset + 6], b"CD001");
    }

    #[test]
    fn test_build_with_file() {
        let mut builder = IsoBuilder::new("WITHFILE");
        builder.add_file("TEST.TXT", b"Hello, ISO!").unwrap();
        let iso = builder.build().unwrap();

        // Verify ISO is valid
        let pvd_offset = 16 * SECTOR_SIZE;
        assert_eq!(&iso[pvd_offset + 1..pvd_offset + 6], b"CD001");
    }

    #[test]
    fn test_build_bootable_iso() {
        let mut builder = IsoBuilder::new("BOOTABLE");

        // Add a simple boot image
        let boot_data = vec![0xEB, 0xFE]; // Infinite loop (JMP $)
        builder.set_boot_image(BootImage::bios_no_emulation(boot_data));

        let iso = builder.build().unwrap();

        // Check Boot Record at sector 17
        let br_offset = 17 * SECTOR_SIZE;
        assert_eq!(iso[br_offset], 0); // Type code for boot record
        assert_eq!(&iso[br_offset + 1..br_offset + 6], b"CD001");
        assert_eq!(&iso[br_offset + 7..br_offset + 30], b"EL TORITO SPECIFICATION");
    }

    #[test]
    fn test_sectors_for_size() {
        assert_eq!(sectors_for_size(0), 0);
        assert_eq!(sectors_for_size(1), 1);
        assert_eq!(sectors_for_size(2048), 1);
        assert_eq!(sectors_for_size(2049), 2);
        assert_eq!(sectors_for_size(4096), 2);
    }
}
