//! Builders for configuring the El Torito bootable disk image properties

use std::num::NonZeroU16;

use super::BuildError;
use super::directory::FileSource;
use crate::eltorito::{self, BootPlatform, EmulationType};

#[derive(Debug)]
pub struct BootConfigBuilder<'a> {
    default_entry: Option<BootEntryBuilder<'a>>,
    sections: Vec<BootSectionBuilder<'a>>,
}

impl<'a> BootConfigBuilder<'a> {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            default_entry: None,
            sections: Vec::with_capacity(capacity),
        }
    }

    pub fn default_entry(
        &mut self,
        emulation: EmulationType,
        boot_image: FileSource<'a>,
    ) -> &mut BootEntryBuilder<'a> {
        self.default_entry.insert(BootEntryBuilder {
            bootable: true,
            emulation,
            load_size: None,
            boot_image,
        })
    }

    pub fn section(
        &mut self,
        platform: BootPlatform,
        id: [u8; 28],
    ) -> Result<&mut BootSectionBuilder<'a>, BuildError> {
        self.section_with_capacity(platform, id, 1)
    }

    pub fn section_with_capacity(
        &mut self,
        platform: BootPlatform,
        id: [u8; 28],
        capacity: usize,
    ) -> Result<&mut BootSectionBuilder<'a>, BuildError> {
        self.sections.push(BootSectionBuilder {
            platform,
            id,
            entries: Vec::with_capacity(capacity),
        });
        Ok(self.sections.last_mut().unwrap())
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
pub struct BootSectionBuilder<'a> {
    platform: BootPlatform,
    id: [u8; 28],
    entries: Vec<BootEntryBuilder<'a>>,
}

impl<'a> BootSectionBuilder<'a> {
    pub fn entry(
        &mut self,
        emulation: EmulationType,
        boot_image: FileSource<'a>,
    ) -> Result<&mut BootEntryBuilder<'a>, BuildError> {
        self.entries.push(BootEntryBuilder {
            bootable: true,
            emulation,
            load_size: None,
            boot_image,
        });
        Ok(self.entries.last_mut().unwrap())
    }
}

#[derive(Debug)]
pub struct BootEntryBuilder<'a> {
    bootable: bool,
    emulation: EmulationType,
    load_size: Option<NonZeroU16>,
    boot_image: FileSource<'a>,
}

impl BootEntryBuilder<'_> {
    pub fn no_emulation(&mut self) -> &mut Self {
        self.emulation = EmulationType::NoEmulation;
        self
    }

    pub fn floppy_12_emulation(&mut self) -> &mut Self {
        self.emulation = EmulationType::Floppy12;
        self
    }

    pub fn floppy_144_emulation(&mut self) -> &mut Self {
        self.emulation = EmulationType::Floppy144;
        self
    }

    pub fn floppy_288_emulation(&mut self) -> &mut Self {
        self.emulation = EmulationType::Floppy288;
        self
    }

    pub fn hard_disk_emulation(&mut self) -> &mut Self {
        self.emulation = EmulationType::HardDisk;
        self
    }
}
