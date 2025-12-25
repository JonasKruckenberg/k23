#![cfg_attr(not(any(test, feature = "emulate")), no_std)]
// #![no_std]
#![feature(step_trait)]
#![feature(debug_closure_helpers)]
#![feature(allocator_api)]

mod address;
mod address_range;
mod address_space;
pub mod arch;
pub mod bootstrap;
mod flush;
mod frame_allocator;
mod memory_attributes;
mod physmap;
mod table;
mod utils;

#[cfg(feature = "emulate")]
mod emulate;

pub use address::{PhysicalAddress, VirtualAddress};
pub use address_range::AddressRangeExt;
pub use address_space::HardwareAddressSpace;
pub use frame_allocator::{AllocError, FrameAllocator, FrameIter};
pub use memory_attributes::{MemoryAttributes, WriteOrExecute};
pub use physmap::PhysicalMemoryMapping;

pub const KIB: usize = 1024;
pub const MIB: usize = KIB * 1024;
pub const GIB: usize = MIB * 1024;
#[cfg(target_pointer_width = "64")]
pub const TIB: usize = GIB * 1024;
