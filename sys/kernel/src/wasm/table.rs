// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use anyhow::Context;
use cranelift_entity::packed_option::ReservedValue;

use crate::wasm::indices::DefinedTableIndex;
use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::types::TableType;
use crate::wasm::values::Ref;
use crate::wasm::vm::{ExportedTable, InstanceAndStore, TableElement, VMTableImport, VmPtr};
use crate::wasm::{Func, vm};

#[derive(Clone, Copy, Debug)]
pub struct Table(Stored<ExportedTable>);

impl Table {
    pub fn new(store: &mut StoreOpaque, ty: TableType, init: Ref) -> crate::Result<Table> {
        let wasm_ty = ty.to_wasm_table();

        // Safety: TODO
        let mut t = unsafe {
            store
                .alloc_mut()
                .allocate_table(wasm_ty, DefinedTableIndex::reserved_value())?
        };

        let init = init.into_table_element(store, ty.element())?;
        t.fill(0, init, ty.minimum())?;

        let definition = store.add_host_table(t);

        let stored = store.add_table(ExportedTable {
            definition,
            vmctx: store.default_caller(),
            table: wasm_ty.clone(),
        });
        Ok(Self(stored))
    }

    pub fn ty(self, store: &StoreOpaque) -> TableType {
        let export = &store[self.0];
        TableType::from_wasm_table(store.engine(), &export.table)
    }

    pub fn get(self, store: &mut StoreOpaque, index: u64) -> Option<Ref> {
        // Safety: TODO
        let table = unsafe { self.vmtable(store).as_mut() };
        let element = table.get(index)?;

        match element {
            TableElement::FuncRef(Some(func_ref)) => {
                // Safety: TODO
                let f = unsafe { Func::from_vm_func_ref(store, func_ref) };
                Some(Ref::Func(Some(f)))
            }
            TableElement::FuncRef(None) => Some(Ref::Func(None)),
        }
    }

    pub fn set(self, store: &mut StoreOpaque, index: u64, val: Ref) -> crate::Result<()> {
        let ty = self.ty(store);
        let val = val.into_table_element(store, ty.element())?;
        // Safety: TODO
        let table = unsafe { self.vmtable(store).as_mut() };
        table.set(index, val)?;

        Ok(())
    }

    pub fn size(self, store: &StoreOpaque) -> u64 {
        // Safety: TODO
        unsafe {
            u64::try_from(
                store[self.0]
                    .definition
                    .as_ref()
                    .current_elements
                    .load(Ordering::Relaxed),
            )
            .unwrap()
        }
    }

    pub fn grow(self, store: &mut StoreOpaque, delta: u64, init: Ref) -> crate::Result<u64> {
        let ty = self.ty(store);
        let init = init.into_table_element(store, ty.element())?;
        // Safety: TODO
        let table = unsafe { self.vmtable(store).as_mut() };
        let old_size = table.grow(delta, init)?.context("failed to grow table")?;
        Ok(u64::try_from(old_size).unwrap())
    }

    pub fn copy(
        store: &mut StoreOpaque,
        dst_table: &Table,
        dst_index: u64,
        src_table: &Table,
        src_index: u64,
        len: u64,
    ) -> crate::Result<()> {
        let dst_ty = dst_table.ty(store);
        let src_ty = src_table.ty(store);

        src_ty
            .element()
            .ensure_matches(store.engine(), dst_ty.element())
            .context(
                "type mismatch: source table's element type does not match \
                 destination table's element type",
            )?;

        let dst_table = dst_table.vmtable(store);
        let src_table = src_table.vmtable(store);

        vm::Table::copy(
            dst_table.as_ptr(),
            src_table.as_ptr(),
            dst_index,
            src_index,
            len,
        )?;

        Ok(())
    }

    pub fn fill(self, store: &mut StoreOpaque, dst: u64, val: Ref, len: u64) -> crate::Result<()> {
        let ty = self.ty(store);
        let val = val.into_table_element(store, ty.element())?;
        // Safety: TODO
        let table = unsafe { self.vmtable(store).as_mut() };
        table.fill(dst, val, len)?;

        Ok(())
    }

    pub(super) fn from_exported_table(store: &mut StoreOpaque, export: ExportedTable) -> Self {
        let stored = store.add_table(export);
        Self(stored)
    }
    pub(super) fn vmtable(self, store: &mut StoreOpaque) -> NonNull<vm::Table> {
        let ExportedTable {
            definition, vmctx, ..
        } = store[self.0];
        // Safety: TODO
        unsafe {
            InstanceAndStore::from_vmctx(vmctx, |pair| {
                let (instance, _) = pair.unpack_mut();
                let def_index = instance.table_index(definition.as_ref());
                instance.get_defined_table(def_index)
            })
        }
    }
    pub(super) fn as_vmtable_import(self, store: &mut StoreOpaque) -> VMTableImport {
        let export = &store[self.0];
        VMTableImport {
            from: VmPtr::from(export.definition),
            vmctx: VmPtr::from(export.vmctx),
        }
    }
}
