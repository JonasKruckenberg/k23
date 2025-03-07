// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::runtime::VMTableImport;
use crate::wasm::store::Stored;
use crate::wasm::{Store, runtime};

/// A WebAssembly table instance.
#[derive(Debug, Clone, Copy)]
pub struct Table(Stored<runtime::ExportedTable>);

impl Table {
    // pub fn new(_store: &mut Store, _ty: TableType, _init: ()) -> crate::wasm::Result<Self> {
    //     todo!()
    // }
    // pub fn ty(&self, _store: &Store) -> &TableType {
    //     todo!()
    // }
    pub(super) fn as_vmtable_import(&self, store: &Store) -> VMTableImport {
        VMTableImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }

    /// # Safety
    ///
    /// The caller must ensure `export` is a valid exported table within `store`.
    pub(super) unsafe fn from_vm_export(store: &mut Store, export: runtime::ExportedTable) -> Self {
        Self(store.push_table(export))
    }

    pub fn comes_from_same_store(self, store: &Store) -> bool {
        store.has_table(self.0)
    }
}
