use crate::runtime::vmcontext::VMContext;
use core::arch::asm;

/// Returns an index for wasm's `memory.grow` builtin function.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn memory32_grow(vmctx: *mut VMContext, delta: u64, index: u32) -> *mut u8 {
    todo!()
}

/// Returns an index for wasm's `table.copy` when both tables are locally
/// defined.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn table_copy(
    vmctx: *mut VMContext,
    dst_index: u32,
    src_index: u32,
    dst: u32,
    src: u32,
    len: u32,
) {
    todo!()
}

/// Returns an index for wasm's `table.init`.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn table_init(
    vmctx: *mut VMContext,
    table: u32,
    elem: u32,
    dst: u32,
    src: u32,
    len: u32,
) {
    todo!()
}

/// Returns an index for wasm's `elem.drop`.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn elem_drop(vmctx: *mut VMContext, elem: u32) {
    todo!()
}

/// Returns an index for wasm's `memory.copy`
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn memory_copy(
    vmctx: *mut VMContext,
    dst_index: u32,
    dst: u64,
    src_index: u32,
    src: u64,
    len: u64,
) {
    todo!()
}

/// Returns an index for wasm's `memory.fill` instruction.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn memory_fill(
    vmctx: *mut VMContext,
    memory: u32,
    dst: u64,
    val: u32,
    len: u64,
) {
    todo!()
}

/// Returns an index for wasm's `memory.init` instruction.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn memory_init(
    vmctx: *mut VMContext,
    memory: u32,
    data: u32,
    dst: u64,
    src: u32,
    len: u32,
) {
    todo!()
}

/// Returns a value for wasm's `ref.func` instruction.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn ref_func(vmctx: *mut VMContext, func: u32) -> *mut u8 {
    todo!()
}

/// Returns an index for wasm's `data.drop` instruction.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn data_drop(vmctx: *mut VMContext, data: u32) {
    todo!()
}

/// Returns a table entry after lazily initializing it.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn table_get_lazy_init_func_ref(
    vmctx: *mut VMContext,
    table: u32,
    index: u32,
) -> *mut u8 {
    todo!()
}

/// Returns an index for Wasm's `table.grow` instruction for `funcref`s.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn table_grow_func_ref(
    vmctx: *mut VMContext,
    table: u32,
    delta: u32,
    init: *mut u8,
) -> u32 {
    todo!()
}

/// Returns an index for Wasm's `table.fill` instruction for `funcref`s.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn table_fill_func_ref(
    vmctx: *mut VMContext,
    table: u32,
    dst: u32,
    val: *mut u8,
    len: u32,
) {
    todo!()
}

/// Returns an index for wasm's `memory.atomic.notify` instruction.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn memory_atomic_notify(
    vmctx: *mut VMContext,
    memory: u32,
    addr: u64,
    count: u32,
) -> u32 {
    todo!()
}

/// Returns an index for wasm's `memory.atomic.wait32` instruction.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn memory_atomic_wait32(
    vmctx: *mut VMContext,
    memory: u32,
    addr: u64,
    expected: u32,
    timeout: u64,
) -> u32 {
    todo!()
}

/// Returns an index for wasm's `memory.atomic.wait64` instruction.
#[link_section = ".text.builtins"]
pub unsafe extern "C" fn memory_atomic_wait64(
    vmctx: *mut VMContext,
    memory: u32,
    addr: u64,
    expected: u64,
    timeout: u64,
) -> u32 {
    todo!()
}

// /// Invoked before malloc returns.
// #[link_section = ".text.builtins"]
// pub unsafe extern "C" fn check_malloc(vmctx: *mut VMContext, addr: u32, len: u32) -> u32 {
//     todo!()
// }
//
// /// Invoked before the free returns.
// #[link_section = ".text.builtins"]
// pub unsafe extern "C" fn check_free(vmctx: *mut VMContext, addr: u32) -> u32 {
//     todo!()
// }
//
// /// Invoked before a load is executed.
// #[link_section = ".text.builtins"]
// pub unsafe extern "C" fn check_load(
//     vmctx: *mut VMContext,
//     num_bytes: u32,
//     addr: u32,
//     offset: u32,
// ) -> u32 {
//     todo!()
// }
//
// /// Invoked before a store is executed.
// #[link_section = ".text.builtins"]
// pub unsafe extern "C" fn check_store(
//     vmctx: *mut VMContext,
//     num_bytes: u32,
//     addr: u32,
//     offset: u32,
// ) -> u32 {
//     todo!()
// }
//
// /// Invoked after malloc is called.
// #[link_section = ".text.builtins"]
// pub unsafe extern "C" fn malloc_start(vmctx: *mut VMContext) {
//     todo!()
// }
//
// /// Invoked after free is called.
// pub unsafe extern "C" fn free_start(vmctx: *mut VMContext) {
//     todo!()
// }
//
// /// Invoked when wasm stack pointer is updated.
// pub unsafe extern "C" fn update_stack_pointer(vmctx: *mut VMContext, value: u32) {
//     todo!()
// }
//
// /// Invoked before memory.grow is called.
// pub unsafe extern "C" fn update_mem_size(vmctx: *mut VMContext, num_bytes: u32) {
//     todo!()
// }
//
// /// Drop a non-stack GC reference (eg an overwritten table entry)
// /// once it will no longer be used again. (Note: `val` is not a
// /// `reference` because it needn't appear in any stack maps, as it
// /// must not be live after this call.)
// pub unsafe extern "C" fn drop_gc_ref(vmctx: *mut VMContext, val: *mut u8) {
//     todo!()
// }
//
// /// Do a GC, treating the optional `root` as a GC root and returning
// /// the updated `root` (so that, in the case of moving collectors,
// /// callers have a valid version of `root` again).
// pub unsafe extern "C" fn gc(vmctx: *mut VMContext, root: *mut u8) -> *mut u8 {
//     todo!()
// }
//
// /// Implementation of Wasm's `global.get` instruction for globals
// /// containing GC references.
// pub unsafe extern "C" fn gc_ref_global_get(vmctx: *mut VMContext, global: u32) -> *mut u8 {
//     todo!()
// }
//
// /// Implementation of Wasm's `global.set` instruction for globals
// /// containing GC references.
// pub unsafe extern "C" fn gc_ref_global_set(vmctx: *mut VMContext, global: u32, val: *mut u8) {
//     todo!()
// }
//
// /// Returns an index for Wasm's `table.grow` instruction for GC references.
// pub unsafe extern "C" fn table_grow_gc_ref(
//     vmctx: *mut VMContext,
//     table: u32,
//     delta: u32,
//     init: *mut u8,
// ) -> u32 {
//     todo!()
// }
//
// /// Returns an index for Wasm's `table.fill` instruction for GC references.
// pub unsafe extern "C" fn table_fill_gc_ref(
//     vmctx: *mut VMContext,
//     table: u32,
//     dst: u32,
//     val: *mut u8,
//     len: u32,
// ) {
//     todo!()
// }
