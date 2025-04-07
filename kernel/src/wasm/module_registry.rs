// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::collections::btree_map::Entry;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use crate::wasm::Module;
use crate::wasm::vm::CodeObject;

/// Used for registering modules with a store.
///
/// Note that the primary reason for this registry is to ensure that everything
/// in `Module` is kept alive for the duration of a `Store`. At this time we
/// need "basically everything" within a `Module` to stay alive once it's
/// instantiated within a store. While there's some smaller portions that could
/// theoretically be omitted as they're not needed by the store they're
/// currently small enough to not worry much about.
#[derive(Default)]
pub struct ModuleRegistry {
    /// Keyed by the end address of a `CodeObject`.
    ///
    /// The value here is the start address and the information about what's
    /// loaded at that address.
    loaded_code: BTreeMap<usize, (usize, LoadedCode)>,

    /// Preserved for keeping data segments alive or similar
    modules_without_code: Vec<Module>,
}

struct LoadedCode {
    /// Kept alive here in the store to have a strong reference to keep the
    /// relevant code mapped while the store is alive.
    _code: Arc<CodeObject>,
    /// Modules found within `self.code`, keyed by start address here of the
    /// address of the first function in the module.
    modules: BTreeMap<usize, Module>,
}

/// An identifier of a module that has previously been inserted into a
/// `ModuleRegistry`.
#[derive(Debug, Clone, Copy)]
pub enum RegisteredModuleId {
    /// Index into `ModuleRegistry::modules_without_code`.
    WithoutCode(usize),
    /// Start address of the module's code so that we can get it again via
    /// `ModuleRegistry::lookup_module`.
    LoadedCode(usize),
}

impl ModuleRegistry {
    /// Get a previously-registered module by id.
    pub fn lookup_module_by_id(&self, id: RegisteredModuleId) -> Option<&Module> {
        match id {
            RegisteredModuleId::WithoutCode(idx) => self.modules_without_code.get(idx),
            RegisteredModuleId::LoadedCode(pc) => {
                let (module, _) = self.module_and_offset(pc)?;
                Some(module)
            }
        }
    }

    /// Registers a new module with the registry.
    pub fn register_module(&mut self, module: &Module) -> RegisteredModuleId {
        self.register(module.code(), Some(module)).unwrap()
    }

    /// Registers a new module with the registry.
    fn register(
        &mut self,
        code: &Arc<CodeObject>,
        module: Option<&Module>,
    ) -> Option<RegisteredModuleId> {
        let text = code.text();

        // If there's not actually any functions in this module then we may
        // still need to preserve it for its data segments. Instances of this
        // module will hold a pointer to the data stored in the module itself,
        // and for schemes that perform lazy initialization which could use the
        // module in the future. For that reason we continue to register empty
        // modules and retain them.
        if text.is_empty() {
            return module.map(|module| {
                let id = RegisteredModuleId::WithoutCode(self.modules_without_code.len());
                self.modules_without_code.push(module.clone());
                id
            });
        }

        // The module code range is exclusive for end, so make it inclusive as
        // it may be a valid PC value
        let start_addr = text.as_ptr() as usize;
        let end_addr = start_addr + text.len() - 1;
        let id = module.map(|_| RegisteredModuleId::LoadedCode(start_addr));

        // If this module is already present in the registry then that means
        // it's either an overlapping image, for example for two modules
        // found within a component, or it's a second instantiation of the same
        // module. Delegate to `push_module` to find out.
        if let Some((other_start, prev)) = self.loaded_code.get_mut(&end_addr) {
            assert_eq!(*other_start, start_addr);
            if let Some(module) = module {
                prev.push_module(module);
            }
            return id;
        }

        // Assert that this module's code doesn't collide with any other
        // registered modules
        if let Some((_, (prev_start, _))) = self.loaded_code.range(start_addr..).next() {
            assert!(*prev_start > end_addr);
        }
        if let Some((prev_end, _)) = self.loaded_code.range(..=start_addr).next_back() {
            assert!(*prev_end < start_addr);
        }

        let mut item = LoadedCode {
            _code: code.clone(),
            modules: Default::default(),
        };
        if let Some(module) = module {
            item.push_module(module);
        }
        let prev = self.loaded_code.insert(end_addr, (start_addr, item));
        assert!(prev.is_none());
        id
    }

    fn module_and_offset(&self, pc: usize) -> Option<(&Module, usize)> {
        let (code, offset) = self.code(pc)?;
        Some((code.module(pc)?, offset))
    }

    fn code(&self, pc: usize) -> Option<(&LoadedCode, usize)> {
        let (end, (start, code)) = self.loaded_code.range(pc..).next()?;
        if pc < *start || *end < pc {
            return None;
        }
        Some((code, pc - *start))
    }
}

impl LoadedCode {
    fn push_module(&mut self, module: &Module) {
        let func = match module.functions().next() {
            Some((_, func)) => func,
            // There are no compiled functions in this module so there's no
            // need to push onto `self.modules` which is only used for frame
            // information lookup for a trap which only symbolicates defined
            // functions.
            None => return,
        };

        let start = module.code().resolve_function_loc(func.wasm_func_loc);

        match self.modules.entry(start) {
            // This module is already present, and it should be the same as
            // `module`.
            Entry::Occupied(m) => {
                debug_assert!(module.same(m.get()));
            }
            // This module was not already present, so now it's time to insert.
            Entry::Vacant(v) => {
                v.insert(module.clone());
            }
        }
    }

    fn module(&self, pc: usize) -> Option<&Module> {
        // The `modules` map is keyed on the start address of the first
        // function in the module, so find the first module whose start address
        // is less than the `pc`. That may be the wrong module but lookup
        // within the module should fail in that case.
        let (_start, module) = self.modules.range(..=pc).next_back()?;
        Some(module)
    }
}