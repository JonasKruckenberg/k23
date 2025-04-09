// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::types::MemoryType;
use crate::wasm::vm::{ExportedMemory, VMMemoryImport, VmPtr};

#[derive(Clone, Copy, Debug)]
pub struct Memory(Stored<ExportedMemory>);

impl Memory {
    pub fn ty(&self, store: &StoreOpaque) -> MemoryType {
        let export = &store[self.0];
        MemoryType::from_wasm_memory(&export.memory)
    }

    pub(super) fn from_exported_memory(store: &mut StoreOpaque, export: ExportedMemory) -> Self {
        let stored = store.add_memory(export);
        Self(stored)
    }
    pub(super) fn as_vmmemory_import(&self, store: &mut StoreOpaque) -> VMMemoryImport {
        let export = &store[self.0];
        VMMemoryImport {
            from: VmPtr::from(export.definition),
            vmctx: VmPtr::from(export.vmctx),
            index: export.index,
        }
    }
}
