//! Builders for creating the on-disk directory hierarchy

use std::borrow::Cow;
use std::collections::{BTreeMap, btree_map};
use std::path::Path;
use std::{fs, io};

use crate::validate::{validate_dir_identifier, validate_file_identifier};

#[derive(Debug)]
pub(super) enum Entry<'a> {
    File(File<'a>),
    Subdir(Directory<'a>),
}

#[derive(Debug)]
pub struct File<'a> {
    pub(crate) content_hash: blake3::Hash,
    pub(crate) source: FileSource<'a>,
}

#[derive(Debug)]
pub enum FileSource<'a> {
    /// Data already in memory.
    InMemory(Cow<'a, [u8]>),
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
}

impl<'a> File<'a> {
    pub fn from_bytes(bytes: impl Into<Cow<'a, [u8]>>) -> Self {
        let bytes = bytes.into();

        let mut hasher = blake3::Hasher::new();
        hasher.update_rayon(&bytes);
        let content_hash = hasher.finalize();

        Self {
            content_hash,
            source: FileSource::InMemory(bytes),
        }
    }

    pub fn from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();

        let mut hasher = blake3::Hasher::new();
        hasher.update_mmap_rayon(path)?;

        let content_hash = hasher.finalize();
        let len = hasher.count();

        let file = fs::File::open(path)?;

        Ok(Self {
            content_hash,
            source: FileSource::OnDisk { len, reader: file },
        })
    }
}

#[derive(Debug, Default)]
pub struct Directory<'a> {
    pub(super) entries: BTreeMap<Cow<'a, str>, Entry<'a>>,
}

impl<'a> Directory<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(
        &mut self,
        name: impl Into<Cow<'a, str>>,
        file: File<'a>,
    ) -> anyhow::Result<&mut Self> {
        let name = name.into();
        validate_file_identifier(name.as_bytes())?;

        match self.entries.entry(name) {
            btree_map::Entry::Occupied(o) => {
                anyhow::bail!("duplicate directory entry: {}", o.key())
            }
            btree_map::Entry::Vacant(v) => {
                v.insert(Entry::File(file));
                Ok(self)
            }
        }
    }

    pub fn add_subdir(
        &mut self,
        name: impl Into<Cow<'a, str>>,
        dir: Directory<'a>,
    ) -> anyhow::Result<&mut Self> {
        let name = name.into();
        validate_dir_identifier(name.as_bytes())?;

        match self.entries.entry(name) {
            btree_map::Entry::Occupied(o) => {
                anyhow::bail!("duplicate directory entry: {}", o.key())
            }
            btree_map::Entry::Vacant(v) => {
                v.insert(Entry::Subdir(dir));
                Ok(self)
            }
        }
    }
}
