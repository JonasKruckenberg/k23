//! States a virtual memory page can be in
//! - Wired - Cannot be paged out or unmapped. This is used for the kernel binary itself, important kernel regions
//!   and other memory that must always be resident in memory.
//! - Reserved - The page belongs to an allocated address space region but is not backed by actual physical memory.
//! - Committed - The page belongs to an allocated address space region and is backed by actual physical memory.

#![cfg_attr(not(test), no_std)]
// #![no_std]

extern crate alloc;

mod address_space;
mod vmo;
