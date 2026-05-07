// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Builders for configuring the El Torito bootable disk image properties

use std::num::NonZeroU16;

use crate::eltorito::{self, BootPlatform, EmulationType};

#[derive(Debug)]
pub struct BootConfig<'a> {
    pub(super) default_entry: BootEntry<'a>,
    pub(super) sections: Vec<BootSection<'a>>,
    /// Platform written into the catalog's `ValidationEntry` header.
    pub(super) validation_platform: BootPlatform,
}

impl<'a> BootConfig<'a> {
    /// Create a new boot catalog. `validation_platform` is the platform id
    /// written into the catalog header (the `ValidationEntry`); `default_entry`
    /// is the El Torito Initial Entry that follows it.
    pub fn new(validation_platform: BootPlatform, default_entry: BootEntry<'a>) -> Self {
        Self {
            default_entry,
            sections: Vec::new(),
            validation_platform,
        }
    }

    pub fn add_section(&mut self, section: BootSection<'a>) -> &mut Self {
        self.sections.push(section);
        self
    }

    pub(crate) fn required_size(&self) -> usize {
        // the two required entries: validation and initial
        let mut len = size_of::<eltorito::ValidationEntry>() + size_of::<eltorito::InitialEntry>();

        // then sum up all the sections
        for section in &self.sections {
            // section header
            len += size_of::<eltorito::SectionHeaderEntry>();

            // and section entries
            len += section.entries.len() * size_of::<eltorito::SectionEntry>();

            // FIXME support section entry extensions here
        }

        len
    }
}

#[derive(Debug)]
pub struct BootSection<'a> {
    pub(super) platform: BootPlatform,
    pub(super) id: [u8; 28],
    pub(super) entries: Vec<BootEntry<'a>>,
}

impl<'a> BootSection<'a> {
    pub fn new(platform: BootPlatform, id: [u8; 28]) -> Self {
        Self {
            platform,
            id,
            entries: Vec::new(),
        }
    }

    pub fn add_entry(&mut self, entry: BootEntry<'a>) -> &mut Self {
        self.entries.push(entry);
        self
    }
}

/// A single El Torito boot entry.
#[derive(Debug)]
pub struct BootEntry<'a> {
    pub(super) bootable: bool,
    pub(super) emulation: EmulationType,
    /// Explicit override for the catalog `sector_count` field (in 512-byte
    /// virtual sectors). If `None`, the layout pass auto-computes the size from the boot image.
    pub(super) load_size: Option<NonZeroU16>,
    // must be a `/`-separated path to a file that has been added to the directory tree.
    pub(super) boot_image_path: &'a str,
    /// LBA of the boot image file; 0 until LBA assignment.
    pub(super) boot_image_lba: u32,
    /// Sector count (512-byte virtual sectors) covering the resolved boot
    /// image; 0 until LBA assignment. Only used when `load_size` is None.
    pub(super) boot_image_sector_count: u16,
}

impl<'a> BootEntry<'a> {
    pub fn new(emulation: EmulationType, boot_image_path: &'a str) -> Self {
        Self {
            bootable: true,
            emulation,
            load_size: None,
            boot_image_path,
            boot_image_lba: 0,
            boot_image_sector_count: 0,
        }
    }

    /// Resolved catalog `sector_count` value: caller-supplied `load_size` if
    /// set, otherwise the auto-computed sector count covering the whole image.
    pub(super) fn resolved_sector_count(&self) -> u16 {
        self.load_size
            .map(NonZeroU16::get)
            .unwrap_or(self.boot_image_sector_count)
    }

    pub fn set_bootable(&mut self, bootable: bool) -> &mut Self {
        self.bootable = bootable;
        self
    }

    pub fn set_emulation(&mut self, emulation: EmulationType) -> &mut Self {
        self.emulation = emulation;
        self
    }

    pub fn set_load_size(&mut self, load_size: NonZeroU16) -> &mut Self {
        self.load_size = Some(load_size);
        self
    }
}
