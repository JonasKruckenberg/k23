#![no_std]

mod config;
mod info;

pub use config::LoaderConfig;
pub use info::{BootInfo, MemoryRegion, MemoryRegionKind};
pub use loader_api_macros::entry;
