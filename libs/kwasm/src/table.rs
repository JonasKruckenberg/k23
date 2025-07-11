// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::pin::Pin;

use crate::TableType;
use crate::store::{StoreOpaque, Stored};
use crate::vm::{ExportedTable, VMTableImport};

#[derive(Clone, Copy, Debug)]
pub struct Table(Stored<ExportedTable>);

impl Table {
    pub fn ty(self, store: &StoreOpaque) -> TableType {
        let export = store.get_table(self.0).unwrap();
        TableType::from_wasm(store.engine(), &export.table)
    }

    pub(crate) fn from_exported_table(store: Pin<&mut StoreOpaque>, export: ExportedTable) -> Self {
        let stored = store.add_table(export);
        Self(stored)
    }

    pub(crate) fn as_vmtable_import(&self, store: Pin<&mut StoreOpaque>) -> VMTableImport {
        todo!()
    }
}
