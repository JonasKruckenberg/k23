// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Builders for creating the on-disk directory hierarchy

use std::borrow::Cow;
use std::collections::{BTreeMap, btree_map};
use std::fs;
use std::path::Path;

use crate::validate::{validate_dir_identifier, validate_file_identifier};

#[derive(Debug)]
pub(super) enum Entry<'a> {
    File(File<'a>),
    Subdir(Directory<'a>),
}

#[derive(Debug)]
pub struct File<'a> {
    pub(crate) source: FileSource<'a>,
}

#[derive(Debug)]
pub enum FileSource<'a> {
    /// Data already in memory.
    InMemory { len: u32, bytes: Cow<'a, [u8]> },
    /// Read from an external source at build time.
    OnDisk { len: u32, reader: fs::File },
}

impl<'a> FileSource<'a> {
    pub fn len(&self) -> u32 {
        match self {
            FileSource::InMemory { len, .. } => *len,
            FileSource::OnDisk { len, .. } => *len,
        }
    }
}

impl<'a> File<'a> {
    /// Create a `File` from a in-memory bytes.
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the file sie exceeds `u32::MAX` (ECMA-119 single-extent files are limited to 4 GiB - 1)
    pub fn from_bytes(bytes: impl Into<Cow<'a, [u8]>>) -> anyhow::Result<Self> {
        let bytes = bytes.into();
        let len = u32::try_from(bytes.len())?;

        Ok(Self {
            source: FileSource::InMemory { len, bytes },
        })
    }

    /// Create a `File` from a file on-disk.
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the file cannot be opened
    /// - when the file sie exceeds `u32::MAX` (ECMA-119 single-extent files are limited to 4 GiB - 1)
    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let file = fs::File::open(path)?;
        let len = u32::try_from(file.metadata()?.len())?;

        Ok(Self {
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

    /// Add a file to this directory.
    ///
    /// # Errors
    ///
    /// Returns `Err` when:
    /// - the file name is malformed
    /// - a directory or file with the name already exists in this directory
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

    /// Add a subdirectory to this directory.
    ///
    /// # Errors
    ///
    /// Returns `Err` when
    /// - the directory name is malformed
    /// - a directory or file with the name already exists in this directory
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
