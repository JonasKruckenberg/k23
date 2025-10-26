// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// #![no_std]
#![cfg_attr(not(any(test, feature = "emulate")), no_std)]
#![feature(step_trait)]
#![feature(debug_closure_helpers)]

//! ## Pointer Provenance
//!
//! Modelling multiple address spaces and switches between them using Rusts Pointer Provenance Model
//! is an open question. We do our best to model the safety invariants in this crate in a compatible way,
//! but, you know, its tricky.
//!
//! Here is the current state of the model: We treat each address space as a separate allocation under Rusts
//! provenance model, and every mapping as a "sub-allocation" inheriting its provenance from the
//! address space. When an address space switch occurs all accesses to the old address space's allocation
//! become out-of-bounds. (Note: If this sounds questionable to you, you're right! Fitting multiple address
//! spaces over Rusts memory model is weird, we're still working on clarifying the details:
//! https://github.com/JonasKruckenberg/k23/issues/599.)

mod address;
mod address_range;
mod address_space;
pub mod arch;
mod asid;
mod bootstrap;
mod flush;
pub mod frame_alloc;
mod memory_attributes;
mod memory_mode;
mod table;
#[cfg(test)]
mod test_utils;
mod utils;
mod visitors;

pub use address::{PhysicalAddress, VirtualAddress};
pub use address_range::AddressRangeExt;
pub use address_space::{AddressSpace, Visit, VisitMut};
pub use arch::Arch;
pub use asid::Asid;
pub use flush::Flush;
pub use memory_attributes::{MemoryAttributes, WriteOrExecute};
pub use memory_mode::{
    HasLevels, HasPhysmap, MemoryMode, MemoryModeBuilder, MissingLevels, MissingPhysmap,
    PageTableLevel,
};
pub use table::Table;

pub const KIB: usize = 1024;
pub const MIB: usize = KIB * 1024;
pub const GIB: usize = MIB * 1024;
#[cfg(target_pointer_width = "64")]
pub const TIB: usize = GIB * 1024;
