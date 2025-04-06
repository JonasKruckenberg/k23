//! #k23VM - k23 WebAssembly Virtual Machine

mod builtins;
mod compile;
mod cranelift;
mod engine;
mod func;
mod global;
mod indices;
mod instance;
mod linker;
mod memory;
mod module;
mod store;
mod table;
mod tag;
mod translate;
mod trap;
mod type_registry;
mod types;
mod utils;
mod values;
mod vm;

pub use engine::Engine;
pub use func::Func;
pub use global::Global;
pub use memory::Memory;
pub use store::Store;
pub use table::Table;
pub use tag::Tag;
pub use trap::Trap;
pub use instance::Instance;
pub use module::Module;
use crate::wasm::utils::{enum_accessors, owned_enum_accessors};

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;
/// Maximum size, in bytes, of 32-bit memories (4G).
pub const WASM32_MAX_SIZE: u64 = 1 << 32;
/// Maximum size, in bytes of WebAssembly stacks.
pub const MAX_WASM_STACK: usize = 512 * 1024;

/***************** Settings *******************************************/
/// Whether lowerings for relaxed simd instructions are forced to
/// be deterministic.
pub const RELAXED_SIMD_DETERMINISTIC: bool = false;
/// 2 GiB of guard pages
/// TODO why does this help to eliminate bounds checks?
pub const DEFAULT_OFFSET_GUARD_SIZE: u64 = 0x8000_0000;
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

impl Extern {
    enum_accessors! {
        e
        (Func(&Func) is_func get_func unwrap_func e)
        (Table(&Table) is_table get_table unwrap_table e)
        (Memory(&Memory) is_memory get_memory unwrap_memory e)
        (Global(&Global) is_global get_global unwrap_global e)
    }

    owned_enum_accessors! {
        e
        (Func(Func) into_func e)
        (Table(Table) into_table e)
        (Memory(Memory) into_memory e)
        (Global(Global) into_global e)
    }
}