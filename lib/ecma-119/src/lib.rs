// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(not(feature = "build"), no_std)]

extern crate alloc;

#[cfg(feature = "build")]
pub mod build;
pub mod eltorito;
mod parse;
mod raw;
pub mod validate;

pub use parse::{DirEntryIter, Directory, DirectoryEntry, File, Image, PathTableIter};
pub use raw::*;
