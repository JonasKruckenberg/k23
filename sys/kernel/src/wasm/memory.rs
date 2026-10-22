// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::types::MemoryType;
use crate::wasm::vm::{self, ExportedMemory, VMMemoryImport, VmPtr};

#[derive(Clone, Copy, Debug)]
pub struct Memory(Stored<ExportedMemory>);

impl Memory {
    /// The declared type of this memory, as written in the module.
    ///
    /// Note that host-imposed limits are not reflected here; see
    /// [`Self::maximum_byte_size`] for the effective maximum.
    pub fn ty(self, store: &StoreOpaque) -> MemoryType {
        let export = &store[self.0];
        MemoryType::from_wasm_memory(&export.memory)
    }

    /// The effective maximum size of this memory, in bytes.
    ///
    /// This is the declared maximum intersected with the host cap that was in
    /// force when the memory was instantiated, which is what `memory.grow`
    /// must enforce. `None` means no bound could be represented.
    pub fn maximum_byte_size(self, store: &mut StoreOpaque) -> Option<usize> {
        let export = &store[self.0];
        let (vmctx, index) = (export.vmctx, export.index);
        // Safety: per invariant 5, `vmctx` belongs to an instance owned by
        // `store`; holding `&mut StoreOpaque` guarantees no other reference to
        // that instance is live for the duration of the closure.
        unsafe {
            vm::Instance::from_vmctx(vmctx, |instance| {
                instance.memories[index].maximum_byte_size()
            })
        }
    }

    pub(super) fn from_exported_memory(store: &mut StoreOpaque, export: ExportedMemory) -> Self {
        let stored = store.add_memory(export);
        Self(stored)
    }
    pub(super) fn as_vmmemory_import(self, store: &mut StoreOpaque) -> VMMemoryImport {
        let export = &store[self.0];
        VMMemoryImport {
            from: VmPtr::from(export.definition),
            vmctx: VmPtr::from(export.vmctx),
            index: export.index,
        }
    }
}
