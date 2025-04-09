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
mod code_registry;
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
pub mod trap_handler;

use crate::scheduler::scheduler;
use crate::shell::Command;
use crate::wasm::store::StoreOpaque;
use crate::wasm::utils::{enum_accessors, owned_enum_accessors};
use crate::wasm::vm::{ConstExprEvaluator, PlaceholderAllocatorDontUse};
use alloc::boxed::Box;
pub use engine::Engine;
pub use func::{Func, TypedFunc};
pub use global::Global;
pub use instance::Instance;
pub use linker::Linker;
pub use memory::Memory;
pub use module::Module;
pub use store::Store;
pub use table::Table;
pub use tag::Tag;
pub use trap::TrapKind;
use wasmparser::Validator;

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
    pub(crate) unsafe fn from_export(export: vm::Export, store: &mut StoreOpaque) -> Self {
        // Safety: ensured by caller
        unsafe {
            match export {
                vm::Export::Function(e) => Extern::Func(Func::from_exported_function(store, e)),
                vm::Export::Table(e) => Extern::Table(Table::from_exported_table(store, e)),
                vm::Export::Memory(e) => Extern::Memory(Memory::from_exported_memory(store, e)),
                vm::Export::Global(e) => Extern::Global(Global::from_exported_global(store, e)),
                vm::Export::Tag(e) => Extern::Tag(Tag::from_exported_tag(store, e)),
            }
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

pub const FIB_TEST: Command = Command::new("wasm-fib-test")
    .with_help("run the WASM test payload")
    .with_fn(|_| {
        fib_test();
        Ok(())
    });

pub const HOSTFUNC_TEST: Command = Command::new("wasm-hostfunc-test")
    .with_help("run the WASM test payload")
    .with_fn(|_| {
        hostfunc_test();
        Ok(())
    });

fn fib_test() {
    use vm::ConstExprEvaluator;
    use wasmparser::Validator;

    let engine = Engine::default();
    let mut validator = Validator::new();
    let mut linker = Linker::new(&engine);
    let mut store = Store::new(&engine, Box::new(PlaceholderAllocatorDontUse), ());
    let mut const_eval = ConstExprEvaluator::default();

    // instantiate & define the fib_cpp module
    {
        let module = Module::from_bytes(
            &engine,
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

    // assert that we correctly parsed the fib export
    assert!(linker.get(&mut store, "fib_cpp", "fib").unwrap().is_func());

    // the module also exports its memory, assert that we correctly parsed that too
    assert!(
        linker
            .get(&mut store, "fib_cpp", "memory")
            .unwrap()
            .is_memory()
    );

    // instantiate the test module
    {
        let module = Module::from_bytes(
            &engine,
            &mut validator,
            include_bytes!("../../fib_test.wasm"),
        )
        .unwrap();

        let instance = linker
            .instantiate(&mut store, &mut const_eval, &module)
            .unwrap();
        instance.debug_vmctx(&store);

        // `fib_test` should only have a single export
        assert_eq!(instance.exports(&mut store).len(), 1);

        let func: TypedFunc<(), ()> = instance
            .get_func(&mut store, "fib_test")
            .unwrap()
            .typed(&store)
            .unwrap();

        scheduler().spawn(
            crate::mem::KERNEL_ASPACE.get().unwrap().clone(),
            async move {
                func.call(&mut store, ()).unwrap();
                tracing::info!("done");
            },
        );
    }
}

fn hostfunc_test() {
    let engine = Engine::default();
    let mut validator = Validator::new();
    let mut linker = Linker::new(&engine);
    let mut store = Store::new(&engine, Box::new(PlaceholderAllocatorDontUse), ());
    let mut const_eval = ConstExprEvaluator::default();

    linker
        .func_wrap("k23", "roundtrip_i64", |arg: u64| -> u64 {
            tracing::debug!("Hello World from hostfunc!");
            arg
        })
        .unwrap();

    let module = Module::from_bytes(
        &engine,
        &mut validator,
        include_bytes!("../../host_func.wasm"),
    )
    .unwrap();

    let instance = linker
        .instantiate(&mut store, &mut const_eval, &module)
        .unwrap();

    instance.debug_vmctx(&store);

    let func: TypedFunc<u64, u64> = instance
        .get_func(&mut store, "roundtrip_i64")
        .unwrap()
        .typed(&mut store)
        .unwrap();

    scheduler().spawn(
        crate::mem::KERNEL_ASPACE.get().unwrap().clone(),
        async move {
            let arg = 42;
            let ret = func.call(&mut store, arg).unwrap();
            assert_eq!(ret, arg);
            tracing::info!("done");
        },
    );

    tracing::info!("success!")
}
