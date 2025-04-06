// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::runtime::{ConstExprEvaluator, Imports};
use crate::wasm::translate::EntityType;
use crate::wasm::{Engine, Extern, Instance, Module, Store};
use alloc::sync::Arc;
use alloc::vec::Vec;
use anyhow::{Context, bail, format_err};
use hashbrown::HashMap;
use hashbrown::hash_map::Entry;

/// A dynamic linker for WebAssembly modules.
#[derive(Debug)]
pub struct Linker {
    engine: Engine,
    string2idx: HashMap<Arc<str>, usize>,
    strings: Vec<Arc<str>>,
    map: HashMap<ImportKey, Extern>,
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
struct ImportKey {
    name: usize,
    module: usize,
}

impl Linker {
    /// Create a new `Linker`.
    ///
    /// This linker is scoped to the provided engine and cannot be used to link modules from other engines.
    pub fn new(engine: &Engine) -> Self {
        Self {
            engine: engine.clone(),
            string2idx: HashMap::new(),
            strings: Vec::new(),
            map: HashMap::new(),
        }
    }

    /// Attempt to retrieve a definition from this linker.
    pub fn get(&self, module: &str, name: &str) -> Option<&Extern> {
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
        store: &mut Store,
        module_name: &str,
        instance: Instance,
    ) -> crate::Result<&mut Self> {
        let exports = instance
            .exports(store)
            .map(|e| (self.import_key(module_name, Some(e.name)), e.value))
            .collect::<Vec<_>>(); // TODO can we somehow get rid of this?

        for (key, ext) in exports {
            self.insert(key, ext)?;
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
        store: &mut Store,
        const_eval: &mut ConstExprEvaluator,
        module: &Module,
    ) -> crate::Result<Instance> {
        let mut imports = Imports::with_capacity_for(module.translated());
        for import in module.imports() {
            let def = self.get(&import.module, &import.name).with_context(|| {
                let type_ = match import.ty {
                    EntityType::Function(_) => "function",
                    EntityType::Table(_) => "table",
                    EntityType::Memory(_) => "memory",
                    EntityType::Global(_) => "global",
                };

                format_err!("Missing {type_} import {}::{}", import.module, import.name)
            })?;

            match (def, &import.ty) {
                (Extern::Func(func), EntityType::Function(_ty)) => {
                    imports.functions.push(func.as_vmfunction_import(store));
                }
                (Extern::Table(table), EntityType::Table(_ty)) => {
                    imports.tables.push(table.as_vmtable_import(store));
                }
                (Extern::Memory(memory), EntityType::Memory(_ty)) => {
                    imports.memories.push(memory.as_vmmemory_import(store));
                }
                (Extern::Global(global), EntityType::Global(_ty)) => {
                    imports.globals.push(global.as_vmglobal_import(store));
                }
                _ => panic!("mismatched import type"),
            }
        }

        // Safety: we have typechecked the imports above.
        unsafe { Instance::new_unchecked(store, const_eval, module.clone(), imports) }
    }

    fn insert(&mut self, key: ImportKey, item: Extern) -> crate::Result<()> {
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
