use crate::wasm_rt::runtime::VMMemoryImport;
use crate::wasm_rt::store::Stored;
use crate::wasm_rt::{runtime, Store};

/// A WebAssembly linear memory instance.
#[derive(Debug, Clone, Copy)]
pub struct Memory(Stored<runtime::ExportedMemory>);

impl Memory {
    // pub fn new(store: &mut Store, ty: MemoryType) -> crate::wasm_rt::Result<Self> {
    //     todo!()
    // }
    // pub fn ty(&self, _store: &Store) -> &MemoryType {
    //     todo!()
    // }
    pub(crate) fn as_vmmemory_import(&self, store: &Store) -> VMMemoryImport {
        VMMemoryImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }
    pub(crate) fn from_vm_export(store: &mut Store, export: runtime::ExportedMemory) -> Self {
        Self(store.push_memory(export))
    }

    pub(crate) fn comes_from_same_store(self, store: &Store) -> bool {
        store.has_memory(self.0)
    }
}
