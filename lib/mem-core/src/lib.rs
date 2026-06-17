// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(not(any(test, feature = "test_utils")), no_std)]
#![feature(step_trait)]
#![cfg_attr(feature = "test_utils", feature(debug_closure_helpers))]

extern crate core;

mod address;
mod address_range;
mod address_space;
pub mod arch;
mod flush;
mod frame_allocator;
mod memory_attributes;
mod physmap;
mod table;
#[cfg(feature = "test_utils")]
pub mod test_utils;
mod utils;

pub use address::{PhysicalAddress, VirtualAddress};
pub use address_range::AddressRangeExt;
pub use address_space::{Active, Bootstrapping, HardwareAddressSpace};
pub use arch::Arch;
pub use flush::Flush;
pub use frame_allocator::{AllocError, BumpAllocator, DEFAULT_MAX_REGIONS, FrameAllocator};
pub use memory_attributes::{MemoryAttributes, MemoryKind, WriteOrExecute};
pub use physmap::PhysMap;
