// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::vec::Vec;
use core::mem;
use core::pin::Pin;

use crate::indices::EntityIndex;
use crate::store::{StoreOpaque, Stored};
use crate::utils::IteratorExt;
use crate::vm::{Imports, InstanceHandle};
use crate::{ConstExprEvaluator, Extern, Func, Global, Memory, Module, Store, Table, Tag};

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct Instance(Stored<InstanceData>);

#[derive(Debug)]
pub(super) struct InstanceData {
    pub handle: InstanceHandle,
    /// A lazily-populated list of exports of this instance. The order of
    /// exports here matches the order of the exports in the original
    /// module.
    exports: Vec<Option<Extern>>,
}

#[derive(Clone)]
pub struct Export<'instance> {
    /// The name of the export.
    pub name: &'instance str,
    /// The definition of the export.
    pub definition: Extern,
}

// ===== impl Instance =====

impl Instance {
    pub(crate) unsafe fn new_unchecked<T>(
        store: &mut Store<T>,
        const_eval: &mut ConstExprEvaluator,
        module: Module,
        imports: Imports,
    ) -> crate::Result<Instance> {
        todo!()
    }

    /// Returns the module this instance was instantiated from.
    pub fn module(self, store: &StoreOpaque) -> &Module {
        store.get_instance(self.0).unwrap().handle.module()
    }

    /// Returns an iterator over the exports of this instance.
    pub(crate) fn exports(
        self,
        mut store: Pin<&mut StoreOpaque>,
    ) -> impl ExactSizeIterator<Item = Export<'_>> {
        let exports = &store.get_instance(self.0).unwrap().exports;

        if exports.iter().any(Option::is_none) {
            let module = store.get_instance(self.0).unwrap().handle.module().clone();

            for name in module.translated().exports.keys() {
                if let Some((export_name_index, _, &entity)) =
                    module.translated().exports.get_full(name)
                {
                    self.get_export_inner(store.as_mut(), entity, export_name_index);
                }
            }
        }

        let instance = store.get_instance_mut(self.0).unwrap();
        let module = instance.handle.module();
        module
            .translated()
            .exports
            .iter()
            .zip_eq(&instance.exports)
            .map(|((name, _), export)| Export {
                name,
                definition: export.clone().unwrap(),
            })
    }

    /// Attempts to get an export from this instance.
    pub fn get_export(self, mut store: Pin<&mut StoreOpaque>, name: &str) -> Option<Extern> {
        let (export_name_index, _, index) =
            self.module(&*store).translated().exports.get_full(name)?;
        let index = *index;
        Some(self.get_export_inner(store.as_mut(), index, export_name_index))
    }

    /// Attempts to get an exported `Func` from this instance.
    pub fn get_func(self, store: Pin<&mut StoreOpaque>, name: &str) -> Option<Func> {
        self.get_export(store, name)?.into_func()
    }

    /// Attempts to get an exported `Table` from this instance.
    pub fn get_table(self, store: Pin<&mut StoreOpaque>, name: &str) -> Option<Table> {
        self.get_export(store, name)?.into_table()
    }

    /// Attempts to get an exported `Memory` from this instance.
    pub fn get_memory(self, store: Pin<&mut StoreOpaque>, name: &str) -> Option<Memory> {
        self.get_export(store, name)?.into_memory()
    }

    /// Attempts to get an exported `Global` from this instance.
    pub fn get_global(self, store: Pin<&mut StoreOpaque>, name: &str) -> Option<Global> {
        self.get_export(store, name)?.into_global()
    }

    pub fn get_tag(self, store: Pin<&mut StoreOpaque>, name: &str) -> Option<Tag> {
        self.get_export(store, name)?.into_tag()
    }

    fn get_export_inner(
        self,
        mut store: Pin<&mut StoreOpaque>,
        entity: EntityIndex,
        export_name_index: usize,
    ) -> Extern {
        // Instantiated instances will lazily fill in exports, so we process
        // all that lazy logic here.
        let data = store.get_instance(self.0).unwrap();

        if let Some(export) = &data.exports[export_name_index] {
            return export.clone();
        }

        let instance = store.as_mut().get_instance_mut(self.0).unwrap(); // Reborrow the &mut InstanceHandle
        // Safety: we just took `instance` from the store, so all its exports must also belong to the store
        let item = unsafe {
            Extern::from_export(instance.handle.get_export_by_index(entity), store.as_mut())
        };
        let data = store.get_instance_mut(self.0).unwrap();
        data.exports[export_name_index] = Some(item.clone());
        item
    }

    pub(crate) fn comes_from_same_store(self, store: &StoreOpaque) -> bool {
        store.has_instance(self.0)
    }
}
