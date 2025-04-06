// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr::NonNull;
use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::types::TableType;
use crate::wasm::values::Ref;
use crate::wasm::vm;
use crate::wasm::vm::{ExportedTable, VMTableImport};

#[derive(Clone, Copy, Debug)]
pub struct Table(Stored<ExportedTable>);

impl Table {
    pub fn new(store: &mut StoreOpaque, ty: TableType, init: Ref) -> crate::Result<Table> {
        todo!()
    }

    pub fn ty(&self, store: &StoreOpaque) -> TableType {
        todo!()
    }

    pub fn get(&self, store: &StoreOpaque, index: u64) -> Option<Ref> {
        todo!()
    }

    pub fn set(&self, store: &mut StoreOpaque, index: u64, val: Ref) -> crate::Result<()> {
        todo!()
    }

    pub fn size(&self, store: &StoreOpaque) -> u64 {
        todo!()
    }

    pub fn grow(&self, store: &mut StoreOpaque, delta: u64, init: Ref) -> crate::Result<u64> {
        todo!()
    }

    pub fn copy(
        store: &mut StoreOpaque,
        dst_table: &Table,
        dst_index: u64,
        src_table: &Table,
        src_index: u64,
        len: u64,
    ) -> crate::Result<()> {
        todo!()
    }

    pub fn fill(&self, store: &mut StoreOpaque, dst: u64, val: Ref, len: u64) -> crate::Result<()> {
        let ty = self.ty(store);

        // let val = val.into_table_element(store, ty.element())?;
        // let exported = &store[self.0];

        todo!()
    }

    fn vmtable(&self, store: &mut StoreOpaque) -> NonNull<vm::Table> {
        let ExportedTable { definition, vmctx } = store[self.0];
        unsafe {
            vm::instance::with_instance_and_store(vmctx, |store, instance| {
                let def_index = instance.table_index(definition.as_ref());
                instance.get_defined_table(def_index)
            })
        }
    }

    pub(super) fn as_vmtable_import(&self, store: &mut StoreOpaque) -> VMTableImport {
        todo!()
    }
}
