//! #k23VM - k23 WebAssembly Virtual Machine

#![expect(unused_imports, reason = "TODO")]

extern crate alloc;
extern crate core;

mod builtins;
mod compile;
mod cranelift;
mod engine;
mod errors;
mod func;
mod global;
mod indices;
mod instance;
mod linker;
mod memory;
mod module;
// mod placeholder;
pub(crate) mod backtrace;
mod instance_allocator;
mod runtime;
mod store;
mod table;
mod translate;
mod trap;
mod type_registry;
mod utils;
mod values;

pub use errors::Error;
use wasmparser::Validator;
pub(crate) type Result<T> = core::result::Result<T, Error>;
use crate::vm::{AddressSpace, ArchAddressSpace, VirtualAddress, KERNEL_ASPACE};
use crate::wasm_rt::instance_allocator::PlaceholderAllocatorDontUse;
use crate::{arch, enum_accessors, owned_enum_accessors};
pub use engine::Engine;
pub use func::Func;
pub use global::Global;
pub use instance::Instance;
pub use linker::Linker;
pub use memory::Memory;
pub use module::Module;
pub use runtime::{ConstExprEvaluator, InstanceAllocator};
pub use store::Store;
pub use table::Table;
pub use translate::ModuleTranslator;
pub use values::{Ref, Val};

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

/// An export from a WebAssembly instance.
pub struct Export<'instance> {
    /// The name of the export.
    pub name: &'instance str,
    /// The exported value.
    pub value: Extern,
}

/// A WebAssembly external value which is just any type that can be imported or exported between modules.
#[derive(Clone, Debug)]
pub enum Extern {
    /// A WebAssembly `func` which can be called.
    Func(Func),
    /// A WebAssembly `table` which is an array of `Val` reference types.
    Table(Table),
    /// A WebAssembly linear memory.
    Memory(Memory),
    /// A WebAssembly `global` which acts like a `Cell<T>` of sorts, supporting
    /// `get` and `set` operations.
    Global(Global),
}

impl Extern {
    pub(crate) fn from_export(export: runtime::Export, store: &mut Store) -> Self {
        use runtime::Export;
        match export {
            Export::Function(e) => Extern::Func(Func::from_vm_export(store, e)),
            Export::Table(e) => Extern::Table(Table::from_vm_export(store, e)),
            Export::Memory(e) => Extern::Memory(Memory::from_vm_export(store, e)),
            Export::Global(e) => Extern::Global(Global::from_vm_export(store, e)),
        }
    }

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

#[cold]
pub fn test() {
    let engine = Engine::default();
    let mut validator = Validator::new();
    let mut linker = Linker::new(&engine);
    let mut store = Store::new(&engine);
    let mut const_eval = ConstExprEvaluator::default();
    // let mut aspace = AddressSpace::new_user(2, None).unwrap();

    // instantiate & define the fib_cpp module
    {
        let module = Module::from_bytes(
            &engine,
            &mut store,
            &mut validator,
            include_bytes!("../../fib_cpp.wasm"),
        )
        .unwrap();

        let instance = linker
            .instantiate(&mut store, &mut const_eval, &module)
            .unwrap();
        instance.debug_vmctx(&store);

        linker
            .define_instance(&mut store, "fib_cpp", instance)
            .unwrap();
    }

    // instantiate the test module
    {
        let module = Module::from_bytes(
            &engine,
            &mut store,
            &mut validator,
            include_bytes!("../../fib_test.wasm"),
        )
        .unwrap();

        let instance = linker
            .instantiate(&mut store, &mut const_eval, &module)
            .unwrap();

        instance.debug_vmctx(&store);

        let func = instance.get_func(&mut store, "fib_test").unwrap();
        // TODO replace with checked
        // Safety: WIP
        unsafe {
            func.call_unchecked(&mut store, &[], &mut []).unwrap();
        }
    }
}
