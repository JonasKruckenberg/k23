// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![feature(new_range_api)]

extern crate alloc;

mod builtins;
mod engine;
mod func;
mod global;
mod indices;
mod loom;
mod memory;
mod module;
mod store;
mod table;
mod tag;
mod trap;
mod type_registry;
mod types;
mod utils;
mod values;
mod vm;
mod wasm;

pub type Result<T> = anyhow::Result<T>;
pub use engine::Engine;
pub use func::Func;
pub use global::Global;
pub use memory::Memory;
pub use module::Module;
pub use store::Store;
pub use table::Table;
pub use tag::Tag;
pub use trap::TrapKind;
pub use types::{
    ArrayType, FieldType, Finality, FuncType, GlobalType, HeapType, MemoryType, Mutability,
    RefType, StorageType, StructType, TableType, TagType, ValType,
};
pub use values::{Ref, Val};

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;
/// Maximum size, in bytes, of 32-bit memories (4G).
pub const WASM32_MAX_SIZE: u64 = 1 << 32;
/// 2 GiB of guard pages
/// TODO why does this help to eliminate bounds checks?
pub const DEFAULT_OFFSET_GUARD_SIZE: u64 = 0x8000_0000;
