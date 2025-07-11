// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::Engine;
use crate::wasm::code_registry::{register_code, unregister_code};
use crate::wasm::compile::{CompileInputs, CompiledFunctionInfo};
use crate::wasm::indices::{DefinedFuncIndex, EntityIndex, VMSharedTypeIndex};
use crate::wasm::translate::{Import, ModuleTranslator, TranslatedModule};
use crate::wasm::type_registry::RuntimeTypeCollection;
use crate::wasm::utils::u8_size_of;
use crate::wasm::vm::{CodeObject, MmapVec, VMArrayCallFunction, VMShape, VMWasmCallFunction};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::mem;
use core::ops::DerefMut;
use core::ptr::NonNull;
use wasmparser::{Validator, WasmFeatures};

/// A compiled WebAssembly module, ready to be instantiated.
///
/// It holds all compiled code as well as the module's type information and other metadata (e.g. for
/// trap handling and backtrace information).
///
/// Currently, no form of dynamic tiering is implemented instead all functions are compiled synchronously
/// when the module is created. This is expected to change though.
#[derive(Debug, Clone)]
pub struct Module(Arc<ModuleInner>);

#[derive(Debug)]
struct ModuleInner {
    name: String,
    engine: Engine,
    translated_module: TranslatedModule,
    required_features: WasmFeatures,
    vmshape: VMShape,
    code: Arc<CodeObject>,
    type_collection: RuntimeTypeCollection,
}

impl Drop for ModuleInner {
    fn drop(&mut self) {
        tracing::warn!("Dropping wasm module {}", self.name);
        unregister_code(&self.code);
    }
}

impl Module {
    pub fn from_bytes(
        engine: &Engine,
        validator: &mut Validator,
        bytes: &[u8],
    ) -> crate::Result<Self> {
        let (mut translation, types) = ModuleTranslator::new(validator).translate(bytes)?;

        tracing::debug!("Gathering compile inputs...");
        let function_body_data = mem::take(&mut translation.function_bodies);
        let inputs = CompileInputs::from_module(&translation, &types, function_body_data);

        tracing::debug!("Compiling inputs...");
        let unlinked_outputs = inputs.compile(engine.compiler())?;

        let type_collection = engine.type_registry().register_module_types(engine, types);

        let code = crate::mem::with_kernel_aspace(|aspace| -> crate::Result<_> {
            tracing::debug!("Applying static relocations...");
            let mut code =
                unlinked_outputs.link_and_finish(engine, &translation.module, |code| {
                    tracing::debug!("Allocating new memory map...");
                    MmapVec::from_slice(aspace.clone(), &code)
                })?;

            code.publish(aspace.lock().deref_mut())?;
            Ok(Arc::new(code))
        })?;

        // register this code memory with the trap handler, so we can correctly unwind from traps
        register_code(&code);

        Ok(Self(Arc::new(ModuleInner {
            name: translation
                .module
                .name
                .clone()
                .unwrap_or("<unnamed mystery module>".to_string()),
            engine: engine.clone(),
            vmshape: VMShape::for_module(u8_size_of::<*mut u8>(), &translation.module),
            translated_module: translation.module,
            required_features: translation.required_features,
            code,
            type_collection,
        })))
    }

    /// Returns the modules name if present.
    pub fn name(&self) -> Option<&str> {
        self.translated().name.as_deref()
    }

    /// Returns the modules imports.
    pub fn imports(&self) -> impl ExactSizeIterator<Item = &Import> {
        self.translated().imports.iter()
    }

    /// Returns the modules exports.
    pub fn exports(&self) -> impl ExactSizeIterator<Item = (&str, EntityIndex)> + '_ {
        self.translated()
            .exports
            .iter()
            .map(|(name, index)| (name.as_str(), *index))
    }

    pub(super) fn required_features(&self) -> WasmFeatures {
        self.0.required_features
    }
    pub(super) fn engine(&self) -> &Engine {
        &self.0.engine
    }
    pub(super) fn translated(&self) -> &TranslatedModule {
        &self.0.translated_module
    }
    pub(super) fn vmshape(&self) -> &VMShape {
        &self.0.vmshape
    }
    pub(crate) fn code(&self) -> &Arc<CodeObject> {
        &self.0.code
    }
    pub(crate) fn type_collection(&self) -> &RuntimeTypeCollection {
        &self.0.type_collection
    }
    pub(crate) fn type_ids(&self) -> &[VMSharedTypeIndex] {
        self.0.type_collection.type_map().values().as_slice()
    }
    pub(crate) fn functions(
        &self,
    ) -> cranelift_entity::Iter<'_, DefinedFuncIndex, CompiledFunctionInfo> {
        self.0.code.function_info().iter()
    }

    pub(super) fn array_to_wasm_trampoline(
        &self,
        index: DefinedFuncIndex,
    ) -> Option<VMArrayCallFunction> {
        let loc = self.0.code.function_info()[index].array_to_wasm_trampoline?;
        let raw = self.code().resolve_function_loc(loc);
        // Safety: TODO
        Some(unsafe { mem::transmute::<usize, VMArrayCallFunction>(raw) })
    }

    /// Return the address, in memory, of the trampoline that allows Wasm to
    /// call a array function of the given signature.
    pub(super) fn wasm_to_array_trampoline(
        &self,
        signature: VMSharedTypeIndex,
    ) -> Option<NonNull<VMWasmCallFunction>> {
        let trampoline_shared_ty = self.0.engine.type_registry().get_trampoline_type(signature);
        let trampoline_module_ty = self
            .0
            .type_collection
            .trampoline_type(trampoline_shared_ty)?;

        debug_assert!(
            self.0
                .engine
                .type_registry()
                .borrow(
                    self.0
                        .type_collection
                        .lookup_shared_type(trampoline_module_ty)
                        .unwrap()
                )
                .unwrap()
                .unwrap_func()
                .is_trampoline_type()
        );

        let ptr = self.code().wasm_to_host_trampoline(trampoline_module_ty);

        Some(ptr)
    }

    pub(super) fn function(&self, index: DefinedFuncIndex) -> NonNull<VMWasmCallFunction> {
        let loc = self.0.code.function_info()[index].wasm_func_loc;
        NonNull::new(self.code().resolve_function_loc(loc) as *mut VMWasmCallFunction).unwrap()
    }

    pub(super) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn new_stub(engine: Engine) -> Self {
        let translated_module = TranslatedModule::default();
        Self(Arc::new(ModuleInner {
            name: "<Stub>".to_string(),
            engine: engine.clone(),
            vmshape: VMShape::for_module(u8_size_of::<usize>(), &translated_module),
            translated_module,
            required_features: WasmFeatures::default(),
            code: Arc::new(CodeObject::empty()),
            type_collection: RuntimeTypeCollection::empty(engine),
        }))
    }
}
