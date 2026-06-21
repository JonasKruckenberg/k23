// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! ECMA-119 (ISO 9660) disk image builder

mod boot;
mod directory;
mod layout;

use std::io;
use std::str::FromStr;

pub use boot::{BootConfig, BootEntry, BootSection};
pub use directory::{Directory, File, FileSource};
use zerocopy::FromZeros;

use self::layout::Layout;
use crate::raw::PrimaryVolumeDescriptor;
use crate::{AStr, DStr};

pub struct ImageBuilder<'a> {
    pvd: PrimaryVolumeDescriptor,
    root: Directory<'a>,
    boot: Option<BootConfig<'a>>,
}

impl<'a> ImageBuilder<'a> {
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
    pub fn new() -> Self {
        Self {
            pvd: {
                let mut pvd = PrimaryVolumeDescriptor::new_zeroed();
                pvd.file_structure_version = 1;

                let id = "K23 ECMA-119"; // FIXME pipe through version env!("CARGO_PKG_VERSION"));
                pvd.data_preparer_id =
                    AStr::from_str(id).expect("data_preparer_id string is too long");

                pvd
            },
            root: Directory::new(),
            boot: None,
        }
    }

    /// Sets this image's "System Identifier".
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the identifier is malformed
    pub fn system_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.system_id = AStr::from_str(s)?;
        Ok(self)
    }

    /// Sets this image's "Volume Identifier".
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the identifier is malformed
    pub fn volume_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.volume_id = DStr::from_str(s)?;
        Ok(self)
    }

    /// Sets this image's "Volume Set Identifier".
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the identifier is malformed
    pub fn volume_set_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.volume_set_id = DStr::from_str(s)?;
        Ok(self)
    }

    /// Sets this image's "Publisher Identifier".
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the identifier is malformed
    pub fn publisher_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.publisher_id = AStr::from_str(s)?;
        Ok(self)
    }

    /// Sets this image's "Data Preparer Identifier".
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the identifier is malformed
    pub fn data_preparer_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.data_preparer_id = AStr::from_str(s)?;
        Ok(self)
    }

    /// Sets this image's "Application Identifier".
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the identifier is malformed
    pub fn application_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.application_id = AStr::from_str(s)?;
        Ok(self)
    }

    /// Replace the root directory. Build a `Directory` independently and hand
    /// it in here when it's complete.
    pub fn set_root(&mut self, root: Directory<'a>) -> &mut Self {
        self.root = root;
        self
    }

    /// Attach an El Torito boot catalog. Build a `BootConfig` independently
    /// and hand it in here when it's complete.
    pub fn set_boot_catalog(&mut self, boot: BootConfig<'a>) -> &mut Self {
        self.boot = Some(boot);
        self
    }

    /// Finish building the image and emit it to the provided writer.
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - building the image fails
    /// - writing the image to disk fails
    pub fn finish(self, writer: impl io::Write + io::Seek) -> io::Result<()> {
        let mut layout = Layout::flatten(self.root);
        let mut boot = self.boot;
        layout.assign_lbas(boot.as_mut())?;
        layout.serialize(writer, self.pvd, boot.as_ref())
    }
}
