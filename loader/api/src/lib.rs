// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

mod config;
mod info;

pub use config::LoaderConfig;
pub use info::{BootInfo, MemoryRegion, BootInfoBuilder, MemoryRegionKind, TlsTemplate};
