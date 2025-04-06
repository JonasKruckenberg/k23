// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::runtime::VMGlobalImport;
use crate::wasm::store::Stored;
use crate::wasm::{Store, Val, runtime};

/// A WebAssembly global instance.
#[derive(Debug, Clone, Copy)]
pub struct Global(Stored<runtime::ExportedGlobal>);

impl Global {
    // pub fn new(store: &mut Store, ty: GlobalType) -> crate::Result<Self> {
    //     todo!()
    // }
    // pub fn ty(&self, _store: &Store) -> &GlobalType {
    //     todo!()
    // }
    /// Get the current value of the global.
    pub fn get(self, _store: &Store) -> Val {
        todo!()
    }
    // pub fn set(&self, store: &mut Store, val: Val) {
    //     todo!()
    // }
    pub(super) fn as_vmglobal_import(&self, store: &Store) -> VMGlobalImport {
        VMGlobalImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }

    /// # Safety
    ///
    /// The caller must ensure `export` is a valid exported global within `store`.
    pub(super) unsafe fn from_vm_export(
        store: &mut Store,
        export: runtime::ExportedGlobal,
    ) -> Self {
        Self(store.push_global(export))
    }

    pub fn comes_from_same_store(self, store: &Store) -> bool {
        store.has_global(self.0)
    }
}
