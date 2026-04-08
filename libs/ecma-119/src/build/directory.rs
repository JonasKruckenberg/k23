//! Builders for creating the on-disk directory hierarchy

use std::path::Path;
use std::{fs, io};

use super::BuildError;

#[derive(Debug)]
pub enum FileSource<'a> {
    /// Data already in memory.
    InMemory(&'a [u8]),
    /// Read from an external source at build time.
    OnDisk { len: u64, reader: fs::File },
}

impl<'a> FileSource<'a> {
    pub fn len(&self) -> usize {
        match self {
            FileSource::InMemory(items) => items.len(),
            FileSource::OnDisk { len, .. } => *len as usize,
        }
    }

    pub fn from_bytes(bytes: &'a [u8]) -> Self {
        Self::InMemory(bytes)
    }

    pub fn from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        let len = file.metadata()?.len();

        Ok(Self::OnDisk { len, reader: file })
    }
}

#[derive(Debug, Default)]
pub struct DirectoryBuilder<'a> {
    pub(super) name: &'a str, // TODO should be bytes & validate file identifier
    pub(super) subdirs: Vec<DirectoryBuilder<'a>>,
    pub(super) files: Vec<(&'a str, FileSource<'a>)>,
}

impl<'a> DirectoryBuilder<'a> {
    // TODO add variants for in-memory and from disk and make FileData private
    pub fn add_file(
        &mut self,
        name: &'a str,
        source: FileSource<'a>,
    ) -> Result<&mut Self, BuildError> {
        self.files.push((name, source));
        Ok(self)
    }

    pub fn add_dir(&mut self, name: &'a str) -> Result<&mut DirectoryBuilder<'a>, BuildError> {
        self.subdirs.push(DirectoryBuilder {
            name,
            subdirs: Vec::new(),
            files: Vec::new(),
        });

        Ok(self.subdirs.last_mut().unwrap())
    }
}
