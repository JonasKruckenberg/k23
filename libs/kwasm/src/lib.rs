// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![feature(new_range_api)]
#![feature(allocator_api)]
#![feature(alloc_layout_extra)]

extern crate alloc;

mod alloc;
mod arch;
mod builtins;
mod compile;
mod cranelift;
mod engine;
mod func;
mod global;
mod indices;
mod instance;
mod linker;
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

use core::pin::Pin;

use crate::store::StoreOpaque;
use crate::utils::{enum_accessors, owned_enum_accessors};

pub type Result<T> = anyhow::Result<T>;

pub use engine::Engine;
pub use func::Func;
pub use global::Global;
pub use instance::Instance;
pub use linker::Linker;
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
pub use vm::{ConstExprEvaluator, Mmap, RawMmap, RawMmapVTable};

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;
/// Maximum size, in bytes, of 32-bit memories (4G).
pub const WASM32_MAX_SIZE: u64 = 1 << 32;
// /// 2 GiB of guard pages
// /// TODO why does this help to eliminate bounds checks?
// pub const DEFAULT_OFFSET_GUARD_SIZE: u64 = 0x8000_0000;
/// The absolute maximum size of a memory in bytes
pub const MEMORY_MAX: usize = 1 << 32;
/// The absolute maximum size of a table in elements
pub const TABLE_MAX: usize = 1 << 10;

/// A WebAssembly external value which is just any type that can be imported or exported between modules.
#[derive(Clone, Debug)]
pub enum Extern {
    Func(Func),
    Table(Table),
    Memory(Memory),
    Global(Global),
    Tag(Tag),
}

impl From<Func> for Extern {
    fn from(f: Func) -> Self {
        Extern::Func(f)
    }
}

impl From<Table> for Extern {
    fn from(t: Table) -> Self {
        Extern::Table(t)
    }
}

impl From<Memory> for Extern {
    fn from(m: Memory) -> Self {
        Extern::Memory(m)
    }
}

impl From<Global> for Extern {
    fn from(g: Global) -> Self {
        Extern::Global(g)
    }
}

impl From<Tag> for Extern {
    fn from(t: Tag) -> Self {
        Extern::Tag(t)
    }
}

impl Extern {
    /// # Safety
    ///
    /// The caller must ensure `export` is a valid export within `store`.
    pub(crate) unsafe fn from_export(export: vm::Export, store: Pin<&mut StoreOpaque>) -> Self {
        match export {
            vm::Export::Function(e) => Extern::Func(Func::from_exported_function(store, e)),
            vm::Export::Table(e) => Extern::Table(Table::from_exported_table(store, e)),
            vm::Export::Memory(e) => Extern::Memory(Memory::from_exported_memory(store, e)),
            vm::Export::Global(e) => Extern::Global(Global::from_exported_global(store, e)),
            vm::Export::Tag(e) => Extern::Tag(Tag::from_exported_tag(store, e)),
        }
    }

    enum_accessors! {
        e
        (Func(&Func) is_func get_func unwrap_func e)
        (Table(&Table) is_table get_table unwrap_table e)
        (Memory(&Memory) is_memory get_memory unwrap_memory e)
        (Global(&Global) is_global get_global unwrap_global e)
        (Tag(&Tag) is_tag get_tag unwrap_tag e)
    }

    owned_enum_accessors! {
        e
        (Func(Func) into_func e)
        (Table(Table) into_table e)
        (Memory(Memory) into_memory e)
        (Global(Global) into_global e)
        (Tag(Tag) into_tag e)
    }
}
