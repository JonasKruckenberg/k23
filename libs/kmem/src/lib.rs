#![no_std]
#![feature(step_trait)]

mod address;
mod address_range;

pub use address::{PhysicalAddress, VirtualAddress};
pub use address_range::AddressRangeExt;
