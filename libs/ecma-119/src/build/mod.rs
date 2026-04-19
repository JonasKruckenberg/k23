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
    pub fn new() -> Self {
        Self {
            pvd: {
                let mut pvd = PrimaryVolumeDescriptor::new_zeroed();
                pvd.file_structure_version = 1;

                let id = format!("K23 ECMA-119 V{}", env!("CARGO_PKG_VERSION"));
                pvd.data_preparer_id =
                    AStr::from_str(&id).expect("data_preparer_id string is too long");

                pvd
            },
            root: Directory::new(),
            boot: None,
        }
    }

    pub fn system_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.system_id = AStr::from_str(s)?;
        Ok(self)
    }

    pub fn volume_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.volume_id = DStr::from_str(s)?;
        Ok(self)
    }

    pub fn volume_set_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.volume_set_id = DStr::from_str(s)?;
        Ok(self)
    }

    pub fn publisher_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.publisher_id = AStr::from_str(s)?;
        Ok(self)
    }

    pub fn data_preparer_id(&mut self, s: &str) -> anyhow::Result<&mut Self> {
        self.pvd.data_preparer_id = AStr::from_str(s)?;
        Ok(self)
    }

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

    pub fn finish(self, writer: impl io::Write + io::Seek) -> io::Result<()> {
        let mut layout = Layout::flatten(self.root);
        let mut boot = self.boot;
        layout.assign_lbas(boot.as_mut())?;
        layout.serialize(writer, self.pvd, boot.as_ref())
    }
}
