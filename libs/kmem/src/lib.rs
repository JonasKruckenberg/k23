// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// #![no_std]
#![cfg_attr(not(test), no_std)]
#![feature(step_trait)]
#![feature(alloc_layout_extra)]
#![cfg_attr(test, feature(allocator_api))]

mod address;
mod address_range;
pub mod arch;
mod frame_allocator;
mod hw_aspace;
mod memory_attributes;
#[cfg(test)]
mod test_utils;

pub use address::{PhysicalAddress, VirtualAddress};
pub use address_range::AddressRangeExt;
pub use frame_allocator::{AllocError, FrameAllocator, FrameIter, FrameIterZeroed};
pub use hw_aspace::HardwareAddressSpace;
pub use memory_attributes::{MemoryAttributes, WriteOrExecute};
