// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::func::Func;
use crate::wasm::global::Global;
use crate::wasm::indices::EntityIndex;
use crate::wasm::memory::Memory;
use crate::wasm::runtime::{ConstExprEvaluator, Imports};
use crate::wasm::store::Stored;
use crate::wasm::table::Table;
use crate::wasm::{Export, Extern, Module, Store, runtime};

/// An instantiated WebAssembly module.
///
/// This is the main representation of all runtime state associated with a running WebAssembly module.
///
/// # Instance and `VMContext`
///
/// `Instance` and `VMContext` are essentially two halves of the same data structure. `Instance` is
/// the privileged host-side half responsible for administrating execution, while `VMContext` holds the
/// actual data that is accessed by compiled WASM code.
#[derive(Debug, Clone, Copy)]
pub struct Instance(Stored<runtime::Instance>);

impl Instance {
    /// Instantiates a new `Instance`.
    ///
    /// # Safety
    ///
    /// This functions assumes the provided `imports` have already been validated and typechecked for
    /// compatibility with the `module` being instantiated.
    pub(crate) unsafe fn new_unchecked(
        store: &mut Store,
        const_eval: &mut ConstExprEvaluator,
        module: Module,
        imports: Imports,
    ) -> crate::Result<Self> {
        // Safety: caller has to ensure safety
        let instance =
            unsafe { runtime::Instance::new_unchecked(store, const_eval, module, imports)? };
        let handle = store.push_instance(instance);
        Ok(Self(handle))
    }

    /// Returns the module this instance was instantiated from.
    pub fn module(self, store: &Store) -> &Module {
        store[self.0].module()
    }

    /// Returns an iterator over the exports of this instance.
    pub(crate) fn exports(self, store: &mut Store) -> impl ExactSizeIterator<Item = Export> {
        let exports = &store[self.0].exports;
        if exports.iter().any(Option::is_none) {
            let module = store[self.0].module().clone();

            for name in module.translated().exports.keys() {
                if let Some((export_name_index, _, &entity)) =
                    module.translated().exports.get_full(name)
                {
                    self.get_export_inner(store, entity, export_name_index);
                }
            }
        }

        let instance = &store[self.0];
        let module = instance.module();
        module
            .translated()
            .exports
            .iter()
            .zip(&instance.exports)
            .map(|((name, _), export)| Export {
                name,
                value: export.clone().unwrap(),
            })
    }

    /// Attempts to get an export from this instance.
    pub fn get_export(self, store: &mut Store, name: &str) -> Option<Extern> {
        let (export_name_index, _, index) =
            self.module(store).translated().exports.get_full(name)?;
        Some(self.get_export_inner(store, *index, export_name_index))
    }

    /// Attempts to get an exported `Func` from this instance.
    pub fn get_func(self, store: &mut Store, name: &str) -> Option<Func> {
        self.get_export(store, name)?.into_func()
    }

    /// Attempts to get an exported `Table` from this instance.
    pub fn get_table(self, store: &mut Store, name: &str) -> Option<Table> {
        self.get_export(store, name)?.into_table()
    }

    /// Attempts to get an exported `Memory` from this instance.
    pub fn get_memory(self, store: &mut Store, name: &str) -> Option<Memory> {
        self.get_export(store, name)?.into_memory()
    }

    /// Attempts to get an exported `Global` from this instance.
    pub fn get_global(self, store: &mut Store, name: &str) -> Option<Global> {
        self.get_export(store, name)?.into_global()
    }

    /// Print a debug representation of this instances `VMContext` to the logger.
    pub fn debug_vmctx(self, store: &Store) {
        store[self.0].debug_vmctx();
    }

    fn get_export_inner(
        self,
        store: &mut Store,
        entity: EntityIndex,
        export_name_index: usize,
    ) -> Extern {
        // Instantiated instances will lazily fill in exports, so we process
        // all that lazy logic here.
        let data = &store[self.0];

        if let Some(export) = &data.exports[export_name_index] {
            return export.clone();
        }

        let instance = &mut store[self.0]; // Reborrow the &mut InstanceHandle
        // Safety: we just took `instance` from the store, so all its exports must also belong to the store
        let item = unsafe { Extern::from_export(instance.get_export_by_index(entity), store) };
        let data = &mut store[self.0];
        data.exports[export_name_index] = Some(item.clone());
        item
    }

    pub(crate) fn comes_from_same_store(self, store: &Store) -> bool {
        store.has_instance(self.0)
    }
}
