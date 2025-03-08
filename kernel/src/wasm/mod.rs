//! #k23VM - k23 WebAssembly Virtual Machine

#![expect(unused_imports, reason = "TODO")]

mod builtins;
mod compile;
mod cranelift;
mod engine;
mod errors;
mod func;
mod global;
mod indices;
mod instance;
mod instance_allocator;
mod linker;
mod memory;
mod module;
mod runtime;
mod store;
mod table;
mod translate;
mod trap;
mod trap_handler;
mod type_registry;
mod utils;
mod values;

use crate::scheduler::scheduler;
use crate::vm::frame_alloc::FRAME_ALLOC;
use crate::vm::ArchAddressSpace;
use crate::{enum_accessors, owned_enum_accessors};
use core::fmt::Write;
use wasmparser::Validator;

use crate::shell::Command;
pub use engine::Engine;
pub use errors::Error;
pub use func::{Func, TypedFunc};
pub use global::Global;
pub use instance::Instance;
pub use linker::Linker;
pub use memory::Memory;
pub use module::Module;
pub use runtime::{ConstExprEvaluator, InstanceAllocator};
pub use runtime::{VMContext, VMFuncRef, VMVal};
pub use store::Store;
pub use table::Table;
pub use translate::ModuleTranslator;
pub use trap::Trap;
pub use trap_handler::handle_wasm_exception;
pub use values::Val;

pub(crate) type Result<T> = core::result::Result<T, Error>;

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
    /// # Safety
    ///
    /// The caller must ensure `export` is a valid export within `store`.
    pub(crate) unsafe fn from_export(export: runtime::Export, store: &mut Store) -> Self {
        // Safety: ensured by caller
        unsafe {
            use runtime::Export;
            match export {
                Export::Function(e) => Extern::Func(Func::from_vm_export(store, e)),
                Export::Table(e) => Extern::Table(Table::from_vm_export(store, e)),
                Export::Memory(e) => Extern::Memory(Memory::from_vm_export(store, e)),
                Export::Global(e) => Extern::Global(Global::from_vm_export(store, e)),
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

fn fib_test() {
    let engine = Engine::default();
    let mut validator = Validator::new();
    let mut linker = Linker::new(&engine);
    let mut store = Store::new(&engine, FRAME_ALLOC.get().unwrap());
    let mut const_eval = ConstExprEvaluator::default();

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

        let func: TypedFunc<(), ()> = instance
            .get_func(&mut store, "fib_test")
            .unwrap()
            .typed(&mut store)
            .unwrap();

        scheduler().spawn(store.alloc.0.clone(), async move {
            func.call(&mut store, ()).await.unwrap();
            tracing::info!("done");
        });
    }
}

pub const HOSTFUNC_TEST: Command = Command::new("wasm-hostfunc-test")
    .with_help("run the WASM test payload")
    .with_fn(|_| {
        host_func_test();
        Ok(())
    });

fn host_func_test() {
    let engine = Engine::default();
    let mut validator = Validator::new();
    let mut linker = Linker::new(&engine);
    let mut store = Store::new(&engine, FRAME_ALLOC.get().unwrap());
    let mut const_eval = ConstExprEvaluator::default();

    linker
        .func_wrap(&mut store, "k23", "roundtrip_i64", |arg: u64| -> u64 {
            tracing::debug!("Hello World from hostfunc!");
            arg
        })
        .unwrap();

    let module = Module::from_bytes(
        &engine,
        &mut store,
        &mut validator,
        include_bytes!("../../host_func_test.wasm"),
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

    scheduler().spawn(store.alloc.0.clone(), async move {
        let arg = 42;
        let ret = func.call(&mut store, arg).await.unwrap();
        assert_eq!(ret, arg);
    });
}

// wasm[0]::function[0]
// 0x200000: addi   sp, sp, -16
// 0x200004: sd     ra, 8(sp)
// 0x200008: sd     s0, 0(sp)
// 0x20000c: mv     s0, sp
// 0x200010: ld     t6, 24(a0)
// 0x200014: addi   t6, t6, 32
// 0x200018: bgeu   sp, t6, 0x200020
// 0x20001c: unimp
// 0x20001e: unimp
// 0x200020: addi   sp, sp, -16
// 0x200024: sd     s1, 8(sp)
// 0x200028: mv     s1, a0
// 0x20002c: ld     a3, 88(a0)
// 0x200030: li     a1, 0
// 0x200034: bne    a3, a1, 0x20003c
// 0x200038: unimp
// 0x20003a: unimp
// 0x20003c: mv     a4, s1
// 0x200040: ld     a0, 104(a4)
// 0x200044: mv     a1, s1
// 0x200048: jalr   a3
// 0x20004c: mv     a1, s1
// 0x200050: ld     s1, 8(sp)
// 0x200054: addi   sp, sp, 16
// 0x200058: ld     ra, 8(sp)
// 0x20005c: ld     s0, 0(sp)
// 0x200060: addi   sp, sp, 16
// 0x200064: ret

// wasm[0]::array_to_wasm_trampoline[1]
// 0x200068: addi   sp, sp, -16
// 0x20006c: sd     ra, 8(sp)
// 0x200070: sd     s0, 0(sp)
// 0x200074: mv     s0, sp
// 0x200078: addi   sp, sp, -32
// 0x20007c: sd     s5, 24(sp)
// 0x200080: sd     s6, 16(sp)
// 0x200084: sd     s8, 8(sp)
// 0x200088: li     s5, 0
// 0x20008c: bne    a3, s5, 0x200094
// 0x200090: unimp
// 0x200092: unimp
// 0x200094: mv     s8, a3
// 0x200098: ld     a3, 0(a2)
// 0x20009c: mv     s6, a2
// 0x2000a0: lw     a2, 0(a0)
// 0x2000a4: lui    a5, 493111
// 0x2000a8: addi   a4, a5, -650
// 0x2000ac: beq    a2, a4, 0x2000b4
// 0x2000b0: unimp
// 0x2000b2: unimp
// 0x2000b4: mv     a2, s0
// 0x2000b8: sd     a2, 48(a0)
// 0x2000bc: mv     a2, a3
// 0x2000c0: auipc  ra, 0
// 0x2000c4: jalr   -192(ra)
// 0x2000c8: mv     a3, s8
// 0x2000cc: bne    a3, s5, 0x2000d4
// 0x2000d0: unimp
// 0x2000d2: unimp
// 0x2000d4: mv     a2, s6
// 0x2000d8: sd     a0, 0(a2)
// 0x2000dc: unimp
// 0x2000de: unimp

// signatures[0]::wasm_to_array_trampoline
// 0x2000e0: addi   sp, sp, -16
// 0x2000e4: sd     ra, 8(sp)
// 0x2000e8: sd     s0, 0(sp)
// 0x2000ec: mv     s0, sp
// 0x2000f0: addi   sp, sp, -32
// 0x2000f4: sd     s10, 24(sp)
// 0x2000f8: mv     a4, a2
// 0x2000fc: lw     a2, 0(a1)
// 0x200100: lui    a3, 493111
// 0x200104: addi   a3, a3, -650
// 0x200108: beq    a2, a3, 0x200110
// 0x20010c: unimp
// 0x20010e: unimp
// 0x200110: ld     a2, 0(s0)
// 0x200114: sd     a2, 32(a1)
// 0x200118: ld     a3, 8(s0)
// 0x20011c: sd     a3, 40(a1)
// 0x200120: li     s10, 1
// 0x200124: bnez   s10, 0x20012c
// 0x200128: unimp
// 0x20012a: unimp
// 0x20012c: mv     a2, sp
// 0x200130: mv     a3, a4
// 0x200134: sd     a3, 0(sp)
// 0x200138: ld     a4, 0(a0)
// 0x20013c: li     a3, 1
// 0x200140: jalr   a4
// 0x200144: bnez   s10, 0x20014c
// 0x200148: unimp
// 0x20014a: unimp
// 0x20014c: ld     a0, 0(sp)
// 0x200150: ld     s10, 24(sp)
// 0x200154: addi   sp, sp, 32
// 0x200158: ld     ra, 8(sp)
// 0x20015c: ld     s0, 0(sp)
// 0x200160: addi   sp, sp, 16
// 0x200164: ret