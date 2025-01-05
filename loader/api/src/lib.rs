#![feature(new_range_api)]
#![no_std]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

mod config;
mod info;

pub use config::LoaderConfig;
pub use info::{BootInfo, MemoryRegion, MemoryRegionKind, TlsTemplate};
