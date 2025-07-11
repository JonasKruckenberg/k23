// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::pin::Pin;

use anyhow::{Context, bail, format_err};
use hashbrown::HashMap;
use hashbrown::hash_map::Entry;

use crate::func::{HostFunc, IntoFunc, WasmParams, WasmResults};
use crate::indices::VMSharedTypeIndex;
use crate::store::StoreOpaque;
use crate::vm::Imports;
use crate::wasm::WasmEntityType;
use crate::{
    ConstExprEvaluator, Engine, Extern, Func, Global, GlobalType, Instance, Memory, MemoryType,
    Module, Store, Table, TableType, Tag, TagType,
};

/// A dynamic linker for WebAssembly modules.
#[derive(Debug)]
pub struct Linker<T> {
    engine: Engine,
    string2idx: HashMap<Arc<str>, usize>,
    strings: Vec<Arc<str>>,
    map: HashMap<ImportKey, Definition>,
    _m: PhantomData<T>,
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
struct ImportKey {
    name: usize,
    module: usize,
}

#[derive(Debug, Clone)]
pub(super) enum Definition {
    Func(Func, VMSharedTypeIndex),
    HostFunc(Arc<HostFunc>, VMSharedTypeIndex),
    Global(Global, GlobalType),
    // Note that tables and memories store not only the original type
    // information but additionally the current size of the table/memory, as
    // this is used during linking since the min size specified in the type may
    // no longer be the current size of the table/memory.
    Table(Table, TableType),
    Memory(Memory, MemoryType),
    Tag(Tag, TagType),
}

impl<T> Linker<T> {
    /// Create a new `Linker`.
    ///
    /// This linker is scoped to the provided engine and cannot be used to link modules from other engines.
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            string2idx: HashMap::new(),
            strings: Vec::new(),
            map: HashMap::new(),
            _m: PhantomData,
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn define(
        &mut self,
        store: Pin<&mut StoreOpaque>,
        module: &str,
        name: &str,
        def: impl Into<Extern>,
    ) -> crate::Result<&mut Self> {
        let key = self.import_key(module, Some(name));
        self.insert(key, Definition::new(&*store, def.into()))?;
        Ok(self)
    }

    /// Attempt to retrieve a definition from this linker.
    pub fn get(&self, store: Pin<&mut StoreOpaque>, module: &str, name: &str) -> Option<Extern> {
        // Safety: TODO
        Some(unsafe { self._get(module, name)?.to_extern(store) })
    }

    fn _get(&self, module: &str, name: &str) -> Option<&Definition> {
        let key = ImportKey {
            module: *self.string2idx.get(module)?,
            name: *self.string2idx.get(name)?,
        };
        self.map.get(&key)
    }

    /// Alias all exports of `module` under the name `as_module`.
    ///
    /// # Errors
    ///
    /// TODO
    pub fn alias_module(&mut self, module: &str, as_module: &str) -> crate::Result<&mut Self> {
        let module = self.intern_str(module);
        let as_module = self.intern_str(as_module);
        let items = self
            .map
            .iter()
            .filter(|(key, _def)| key.module == module)
            .map(|(key, def)| (key.name, def.clone()))
            .collect::<Vec<_>>();
        for (name, item) in items {
            self.insert(
                ImportKey {
                    module: as_module,
                    name,
                },
                item,
            )?;
        }
        Ok(self)
    }

    /// Define all exports of the provided `instance` under the module name `module_name`.
    ///
    /// # Errors
    ///
    /// TODO
    pub fn define_instance(
        &mut self,
        mut store: Pin<&mut StoreOpaque>,
        module_name: &str,
        instance: Instance,
    ) -> crate::Result<&mut Self> {
        let exports = instance
            .exports(store.as_mut())
            .map(|e| (self.import_key(module_name, Some(e.name)), e.definition))
            .collect::<Vec<_>>(); // TODO can we somehow get rid of this?

        for (key, ext) in exports {
            self.insert(key, Definition::new(&*store, ext))?;
        }

        Ok(self)
    }

    /// Instantiate the provided `module`.
    ///
    /// This step resolve the modules imports using definitions from this linker, then pass them
    /// on to `Instance::new_unchecked` for instantiation.
    ///
    /// Each import of module will be looked up in this Linker and must have previously been defined.
    ///
    /// # Errors
    ///
    /// TODO
    ///
    /// # Panics
    ///
    /// TODO
    pub fn instantiate(
        &self,
        store: &mut Store<T>,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
    ) -> crate::Result<Instance> {
        let mut imports = Imports::with_capacity_for(module.translated());

        for import in module.imports() {
            let def = self._get(&import.module, &import.name).with_context(|| {
                let type_ = match import.ty {
                    WasmEntityType::Function(_) => "function",
                    WasmEntityType::Table(_) => "table",
                    WasmEntityType::Memory(_) => "memory",
                    WasmEntityType::Global(_) => "global",
                    WasmEntityType::Tag(_) => "tag",
                };

                format_err!("Missing {type_} import {}::{}", import.module, import.name)
            })?;

            match (def, &import.ty) {
                (Definition::Func(func, _actual), WasmEntityType::Function(_expected)) => {
                    imports
                        .functions
                        .push(func.as_vmfunction_import(store.opaque_mut(), module));
                }
                (Definition::HostFunc(func, _actual), WasmEntityType::Function(_expected)) => {
                    let func = func.clone().to_func(store.opaque_mut());
                    imports
                        .functions
                        .push(func.as_vmfunction_import(store.opaque_mut(), module));
                }
                (Definition::Table(table, _actual), WasmEntityType::Table(_expected)) => {
                    imports
                        .tables
                        .push(table.as_vmtable_import(store.opaque_mut()));
                }
                (Definition::Memory(memory, _actual), WasmEntityType::Memory(_expected)) => {
                    imports
                        .memories
                        .push(memory.as_vmmemory_import(store.opaque_mut()));
                }
                (Definition::Global(global, _actual), WasmEntityType::Global(_expected)) => {
                    imports
                        .globals
                        .push(global.as_vmglobal_import(store.opaque_mut()));
                }
                (Definition::Tag(tag, _actual), WasmEntityType::Tag(_expected)) => {
                    imports.tags.push(tag.as_vmtag_import(store.opaque_mut()));
                }
                _ => panic!("mismatched import type"),
            }
        }

        // Safety: we have typechecked the imports above.
        unsafe { Instance::new_unchecked(store, const_eval, module.clone(), imports) }
    }

    pub fn func_wrap<Params, Results>(
        &mut self,
        module: &str,
        name: &str,
        func: impl IntoFunc<T, Params, Results>,
    ) -> crate::Result<&mut Self>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        let (func, ty) = HostFunc::wrap(self.engine(), func);

        let key = self.import_key(module, Some(name));
        self.insert(key, Definition::HostFunc(Arc::new(func), ty.type_index()))?;

        Ok(self)
    }

    fn insert(&mut self, key: ImportKey, item: Definition) -> crate::Result<()> {
        match self.map.entry(key) {
            Entry::Occupied(_) => {
                bail!(
                    "Name {}::{} is already defined",
                    self.strings[key.module],
                    self.strings[key.name]
                );
            }
            Entry::Vacant(v) => {
                v.insert(item);
            }
        }

        Ok(())
    }

    fn import_key(&mut self, module: &str, name: Option<&str>) -> ImportKey {
        ImportKey {
            module: self.intern_str(module),
            name: name.map_or(usize::MAX, |name| self.intern_str(name)),
        }
    }

    fn intern_str(&mut self, string: &str) -> usize {
        if let Some(idx) = self.string2idx.get(string) {
            return *idx;
        }
        let string: Arc<str> = string.into();
        let idx = self.strings.len();
        self.strings.push(string.clone());
        self.string2idx.insert(string, idx);
        idx
    }
}

impl Definition {
    fn new(store: &StoreOpaque, item: Extern) -> Definition {
        match item {
            Extern::Func(f) => Definition::Func(f, f.type_index(store)),
            Extern::Table(t) => Definition::Table(t, t.ty(store)),
            Extern::Memory(m) => Definition::Memory(m, m.ty(store)),
            Extern::Global(g) => Definition::Global(g, g.ty(store)),
            Extern::Tag(t) => Definition::Tag(t, t.ty(store)),
        }
    }

    unsafe fn to_extern(&self, store: Pin<&mut StoreOpaque>) -> Extern {
        match self {
            Definition::Func(f, _) => Extern::Func(*f),
            Definition::HostFunc(f, _) => Extern::Func(f.clone().to_func(store)),
            Definition::Global(g, _) => Extern::Global(*g),
            Definition::Table(t, _) => Extern::Table(*t),
            Definition::Memory(m, _) => Extern::Memory(*m),
            Definition::Tag(t, _) => Extern::Tag(*t),
        }
    }
}
