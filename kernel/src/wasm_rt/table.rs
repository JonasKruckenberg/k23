use crate::wasm_rt::runtime::VMTableImport;
use crate::wasm_rt::store::Stored;
use crate::wasm_rt::{runtime, Store};

/// A WebAssembly table instance.
#[derive(Debug, Clone, Copy)]
pub struct Table(Stored<runtime::ExportedTable>);

impl Table {
    // pub fn new(_store: &mut Store, _ty: TableType, _init: ()) -> crate::wasm_rt::Result<Self> {
    //     todo!()
    // }
    // pub fn ty(&self, _store: &Store) -> &TableType {
    //     todo!()
    // }
    pub(crate) fn as_vmtable_import(&self, store: &Store) -> VMTableImport {
        VMTableImport {
            from: store[self.0].definition,
            vmctx: store[self.0].vmctx,
        }
    }
    pub(crate) fn from_vm_export(store: &mut Store, export: runtime::ExportedTable) -> Self {
        Self(store.push_table(export))
    }
    pub(crate) fn comes_from_same_store(self, store: &Store) -> bool {
        store.has_table(self.0)
    }
}
