//! ECMA-119 (ISO 9660) disk image builder

mod boot;
mod directory;
mod layout;

use std::io;
use std::str::FromStr;

pub use boot::{BootConfigBuilder, BootEntryBuilder, BootSectionBuilder};
pub use directory::{DirectoryBuilder, FileSource};
use zerocopy::FromZeros;

use self::layout::Layout;
use crate::raw::PrimaryVolumeDescriptor;
use crate::validate::ValidationError;
use crate::{AStr, DStr};

#[derive(Debug)]
pub enum BuildError {
    /// A field value failed ECMA-119 validation.
    Invalid(ValidationError),
}

impl core::fmt::Display for BuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BuildError::Invalid(e) => write!(f, "invalid field value: {e}"),
        }
    }
}

impl core::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            BuildError::Invalid(e) => Some(e),
        }
    }
}

impl From<ValidationError> for BuildError {
    fn from(e: ValidationError) -> Self {
        Self::Invalid(e)
    }
}

pub struct ImageBuilder<'a> {
    // TODO primary volume descriptor string fields
    pvd: PrimaryVolumeDescriptor,
    root: DirectoryBuilder<'a>,
    boot: Option<BootConfigBuilder<'a>>,
}

impl<'a> ImageBuilder<'a> {
    pub fn new() -> Self {
        Self {
            pvd: {
                let mut pvd = PrimaryVolumeDescriptor::new_zeroed();
                pvd.file_structure_version = 1;

                let id = format!("k23 ECMA-119 v{}", env!("CARGO_PKG_VERSION"));
                pvd.data_preparer_id = AStr::from_str(&id).unwrap();

                pvd
            },
            root: DirectoryBuilder::default(),
            boot: None,
        }
    }

    // pub volume_space_size: BothEndianU32,
    // pub volume_set_size: BothEndianU16,
    // pub volume_sequence_number: BothEndianU16,
    // pub logical_block_size: BothEndianU16,

    // pub copyright_file_id: FileId<37>,
    // pub abstract_file_id: FileId<37>,
    // pub bibliographic_file_id: FileId<37>,

    // pub creation_date: DecDateTime,
    // pub modification_date: DecDateTime,
    // pub expiration_date: DecDateTime,
    // pub effective_date: DecDateTime,

    pub fn system_identifier(&mut self, s: &str) -> Result<&mut Self, BuildError> {
        self.pvd.system_id = AStr::from_str(s)?;
        Ok(self)
    }

    pub fn volume_identifier(&mut self, s: &str) -> Result<&mut Self, BuildError> {
        self.pvd.volume_id = DStr::from_str(s)?;
        Ok(self)
    }

    pub fn volume_set_identifier(&mut self, s: &str) -> Result<&mut Self, BuildError> {
        self.pvd.volume_set_id = DStr::from_str(s)?;
        Ok(self)
    }

    pub fn publisher_identifier(&mut self, s: &str) -> Result<&mut Self, BuildError> {
        self.pvd.publisher_id = AStr::from_str(s)?;
        Ok(self)
    }

    pub fn data_preparer_identifier(&mut self, s: &str) -> Result<&mut Self, BuildError> {
        self.pvd.data_preparer_id = AStr::from_str(s)?;
        Ok(self)
    }

    pub fn application_identifier(&mut self, s: &str) -> Result<&mut Self, BuildError> {
        self.pvd.application_id = AStr::from_str(s)?;
        Ok(self)
    }

    pub fn boot_catalog(&mut self) -> &mut BootConfigBuilder<'a> {
        self.boot_catalog_with_capacity(1)
    }

    pub fn boot_catalog_with_capacity(&mut self, capacity: usize) -> &mut BootConfigBuilder<'a> {
        self.boot.insert(BootConfigBuilder::with_capacity(capacity))
    }

    pub fn root(&mut self) -> &mut DirectoryBuilder<'a> {
        &mut self.root
    }

    pub fn finish(self, writer: impl io::Write + io::Seek) -> io::Result<()> {
        let mut layout = Layout::flatten(self.root);
        layout.assign_lbas(self.boot.as_ref());
        layout.serialize(writer, self.pvd, self.boot.as_ref())
    }
}
