#![cfg_attr(not(test), no_std)]
extern crate alloc;

mod access_rules;
pub mod address_space;
mod addresses;
mod frame;
pub mod frame_alloc;
#[cfg(test)]
mod test_utils;
mod utils;
mod vmo;
mod test;

pub type Result<T> = anyhow::Result<T>;

pub use access_rules::{AccessRules, WriteOrExecute};
pub use addresses::{AddressRangeExt, PhysicalAddress, VirtualAddress};
pub use frame::{Frame, FrameRef};
