// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![feature(step_trait)]

extern crate core;

mod address;
mod address_range;
pub mod arch;
mod frame_allocator;
mod memory_attributes;
mod physmap;

pub use address::{PhysicalAddress, VirtualAddress};
pub use address_range::AddressRangeExt;
pub use arch::Arch;
pub use frame_allocator::{AllocError, FrameAllocator};
pub use memory_attributes::{MemoryAttributes, MemoryKind, WriteOrExecute};
pub use physmap::PhysMap;
