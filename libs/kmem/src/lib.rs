// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
extern crate alloc;

mod access_rules;
mod address_space;
mod addresses;

pub type Result<T> = anyhow::Result<T>;

pub use access_rules::{AccessRules, WriteOrExecute};
pub use address_space::AddressSpace;
pub use addresses::{AddressRangeExt, PhysicalAddress, VirtualAddress};
