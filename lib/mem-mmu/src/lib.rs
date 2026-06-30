// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! The hardware page-table engine: the single layer in k23 that manipulates
//! architecture page tables. Layered on top of `mem-core`'s memory vocabulary
//! (`Arch`, addresses, `PhysMap`, `FrameAllocator`); the virtual-memory subsystem
//! (`vmem`) is in turn layered on top of this.

#![no_std]

mod address_space;
mod flush;
mod table;
mod utils;

pub use address_space::HardwareAddressSpace;
pub use flush::Flush;
pub use table::{Table, marker};
// Re-exported for the `mem-testkit` emulator and out-of-crate tests; the page-walk
// helper is otherwise an internal detail of this crate.
pub use utils::{PageTableEntries, page_table_entries_for};
