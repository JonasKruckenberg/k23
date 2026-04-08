//! ECMA-119 (ISO 9660) disk image builder

mod boot;
mod directory;
mod layout;

use std::io;

pub use boot::{BootConfigBuilder, BootEntryBuilder, BootSectionBuilder};
pub use directory::{DirectoryBuilder, FileSource};
use zerocopy::FromZeros;

use self::layout::Layout;
use crate::raw::PrimaryVolumeDescriptor;

#[derive(Debug)]
pub enum BuildError {}

pub struct ImageBuilder<'a> {
    // TODO primary volume descriptor string fields
    pvd: PrimaryVolumeDescriptor,
    root: DirectoryBuilder<'a>,
    boot: Option<BootConfigBuilder<'a>>,
}

impl<'a> ImageBuilder<'a> {
    pub fn new() -> Self {
        Self {
            pvd: PrimaryVolumeDescriptor::new_zeroed(),
            root: DirectoryBuilder::default(),
            boot: None,
        }
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
