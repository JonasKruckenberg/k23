//! States a virtual memory page can be in
//! - Wired - Cannot be paged out or unmapped. This is used for the kernel binary itself, important kernel regions
//!   and other memory that must always be resident in memory.
//! - Reserved - The page belongs to an allocated address space region but is not backed by actual physical memory.
//! - Committed - The page belongs to an allocated address space region and is backed by actual physical memory.

#![cfg_attr(not(test), no_std)]
#![feature(new_range_api)]
#![feature(allocator_api)]

extern crate alloc;

mod address_space;
mod error;
mod vmo;

pub use error::Error;
type Result<T> = core::result::Result<T, Error>;
