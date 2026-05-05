// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr::NonNull;

use crate::wasm::TrapKind;
use crate::wasm::indices::{DataIndex, ElemIndex, MemoryIndex, TableIndex};
use crate::wasm::store::StoreOpaque;
use crate::wasm::trap_handler::HostResultHasUnwindSentinel;
use crate::wasm::vm::instance::Instance;
use crate::wasm::vm::table::{TableElement, TableElementType};
use crate::wasm::vm::{Table, VMFuncRef};

/// A helper structure to represent the return value of a memory or table growth
/// call.
///
/// This represents a byte or element-based count of the size of an item on the
/// host. For example a memory is how many bytes large the memory is, or a table
/// is how many elements large it is. It's assumed that the value here is never
/// -1 or -2 as that would mean the entire host address space is allocated which
/// is not possible.
struct AllocationSize(usize);

/// Special implementation for growth-related libcalls.
///
/// Here the optional return value means:
///
/// * `Some(val)` - the growth succeeded and the previous size of the item was
///   `val`.
/// * `None` - the growth failed.
///
/// The failure case returns -1 (or `usize::MAX` as an unsigned integer) and the
/// successful case returns the `val` itself. Note that -2 (`usize::MAX - 1`
/// when unsigned) is unwind as a sentinel to indicate an unwind as no valid
/// allocation can be that large.
// Safety: TODO
unsafe impl HostResultHasUnwindSentinel for Option<AllocationSize> {
    type Abi = *mut u8;
    const SENTINEL: *mut u8 = (usize::MAX - 1) as *mut u8;

    fn into_abi(self) -> *mut u8 {
        match self {
            Some(size) => {
                debug_assert!(size.0 < (usize::MAX - 1));
                size.0 as *mut u8
            }
            None => usize::MAX as *mut u8,
        }
    }
}

// Implementation of `memory.grow`
fn memory_grow(
    store: &mut StoreOpaque,
    instance: &mut Instance,
    memory_index: u32,
    delta: u64,
) -> crate::Result<Option<AllocationSize>> {
    let memory_index = MemoryIndex::from_u32(memory_index);

    let result = instance
        .memory_grow(store, memory_index, delta)?
        .map(|size_in_bytes| {
            AllocationSize(
                usize::try_from(size_in_bytes / instance.memory_page_size(memory_index)).unwrap(),
            )
        });

    Ok(result)
}

// Implementation of `memory.fill`
fn memory_fill(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    memory_index: u32,
    dst: u64,
    val: u32,
    len: u64,
) -> Result<(), TrapKind> {
    let memory_index = MemoryIndex::from_u32(memory_index);
    instance.memory_fill(memory_index, dst, u8::try_from(val).unwrap(), len)
}

// Implementation of `memory.init`
fn memory_init(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    memory_index: u32,
    data_index: u32,
    dst: u64,
    src: u32,
    len: u32,
) -> Result<(), TrapKind> {
    let memory_index = MemoryIndex::from_u32(memory_index);
    let data_index = DataIndex::from_u32(data_index);
    instance.memory_init(memory_index, data_index, dst, src, len)
}

// Implementation of `data.drop`
fn data_drop(_store: &mut StoreOpaque, instance: &mut Instance, data_index: u32) {
    instance.data_drop(DataIndex::from_u32(data_index));
}

// Implementation of `memory.copy`.
fn memory_copy(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    dst_index: u32,
    dst: u64,
    src_index: u32,
    src: u64,
    len: u64,
) -> Result<(), TrapKind> {
    let dst_index = MemoryIndex::from_u32(dst_index);
    let src_index = MemoryIndex::from_u32(src_index);
    instance.memory_copy(dst_index, dst, src_index, src, len)
}

#[expect(
    clippy::cast_ptr_alignment,
    reason = "the cast from u8 to VMFuncRef is fine, *mut u8 is just type-erased"
)]
unsafe fn table_grow_func_ref(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    table_index: u32,
    delta: u64,
    init_value: *mut u8,
) -> crate::Result<Option<AllocationSize>> {
    let table_index = TableIndex::from_u32(table_index);

    let element = match instance.table_element_type(table_index) {
        TableElementType::Func => {
            TableElement::FuncRef(NonNull::new(init_value.cast::<VMFuncRef>()))
        }
        TableElementType::GcRef => unreachable!(),
    };

    let res = instance
        .table_grow(table_index, delta, element)?
        .map(AllocationSize);

    Ok(res)
}

#[expect(
    clippy::cast_ptr_alignment,
    reason = "the cast from u8 to VMFuncRef is fine, *mut u8 is just type-erased"
)]
fn table_fill_func_ref(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    table_index: u32,
    dst: u64,
    val: *mut u8,
    len: u64,
) -> Result<(), TrapKind> {
    let table_index = TableIndex::from_u32(table_index);

    let element = match instance.table_element_type(table_index) {
        TableElementType::Func => TableElement::FuncRef(NonNull::new(val.cast::<VMFuncRef>())),
        TableElementType::GcRef => unreachable!(),
    };

    instance.table_fill(table_index, dst, element, len)
}

// Implementation of `table.copy`.
fn table_copy(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    dst_table_index: u32,
    src_table_index: u32,
    dst: u64,
    src: u64,
    len: u64,
) -> Result<(), TrapKind> {
    let dst_table_index = TableIndex::from_u32(dst_table_index);
    let src_table_index = TableIndex::from_u32(src_table_index);

    let dst_table = instance.defined_or_imported_table(dst_table_index);
    let src_table = instance.defined_or_imported_table(src_table_index);

    // Notice that this actually *doesn't* go through instance like the other table_* builtins
    // This is because copy needs to borrow two tables mutably at the same time (they might be the same table too)
    // which of course is horrifically incompatible with Rusts borrow rules. This (plus passing *mut Table instead of &mut Table)
    // is our way of working around this
    Table::copy(dst_table.as_ptr(), src_table.as_ptr(), dst, src, len)
}

// Implementation of `table.init`.
fn table_init(
    store: &mut StoreOpaque,
    instance: &mut Instance,
    table_index: u32,
    elem_index: u32,
    dst: u64,
    src: u64,
    len: u64,
) -> Result<(), TrapKind> {
    let table_index = TableIndex::from_u32(table_index);
    let elem_index = ElemIndex::from_u32(elem_index);
    instance.table_init(store, table_index, elem_index, dst, src, len)
}

// Implementation of `elem.drop`.
fn elem_drop(_store: &mut StoreOpaque, instance: &mut Instance, elem_index: u32) {
    instance.elem_drop(ElemIndex::from_u32(elem_index));
}

#[expect(unused, reason = "TODO")]
fn memory_atomic_notify(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    memory_index: u32,
    addr_index: u64,
    count: u32,
) -> Result<u32, TrapKind> {
    todo!()
}

#[expect(unused, reason = "TODO")]
fn memory_atomic_wait32(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    memory_index: u32,
    addr_index: u64,
    expected: u32,
    timeout: u64,
) -> Result<u32, TrapKind> {
    todo!()
}

#[expect(unused, reason = "TODO")]
fn memory_atomic_wait64(
    _store: &mut StoreOpaque,
    instance: &mut Instance,
    memory_index: u32,
    addr_index: u64,
    expected: u64,
    timeout: u64,
) -> Result<u32, TrapKind> {
    todo!()
}

fn raise(_store: &mut StoreOpaque, _instance: &mut Instance) {
    tracing::debug!("{_store:?} {_instance:?}");

    todo!()

    // unsafe {
    //     crate::wasm::vm::trap_handler::raise_preexisting_trap()
    // }
}

pub mod raw {
    use crate::wasm::builtins::foreach_builtin_function;

    macro_rules! builtin {
        (
            $(
                $( #[cfg($attr:meta)] )?
                $name:ident( vmctx: vmctx $(, $pname:ident: $param:ident )* ) $(-> $result:ident)?;
            )*
        ) => {
            $(
                pub unsafe extern "C" fn $name(
                    vmctx: ::core::ptr::NonNull<$crate::wasm::vm::VMContext>,
                    $( $pname : builtin!(@ty $param), )*
                ) $(-> builtin!(@ty $result))? {
                    $crate::wasm::trap_handler::catch_unwind_and_record_trap(|| {
                        // Safety: there isn't much we can check here, we receive the vmctx
                        // from generated code, so as long as the compiler is correct, this is safe.
                        unsafe {
                            $crate::wasm::vm::InstanceAndStore::from_vmctx(vmctx, |pair| {
                                let (instance, store) = pair.unpack_mut();
                                super::$name(store, instance, $($pname),*)
                            })
                        }
                    })
                }
            )*
        };

        (@ty u32) => (u32);
        (@ty u64) => (u64);
        (@ty u8) => (u8);
        (@ty bool) => (bool);
        (@ty pointer) => (*mut u8);
    }

    foreach_builtin_function!(builtin);
}
