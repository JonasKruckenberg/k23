// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::compile::CompiledFunctionInfo;
use crate::wasm::indices::{DefinedFuncIndex, EntityIndex, VMSharedTypeIndex};
use crate::wasm::store::StoreOpaque;
use crate::wasm::translate::{Import, TranslatedModule};
use crate::wasm::type_registry::RuntimeTypeCollection;
use crate::wasm::utils::u8_size_of;
use crate::wasm::vm::{CodeObject, VMArrayCallFunction, VMShape, VMWasmCallFunction};
use crate::wasm::Engine;
use alloc::sync::Arc;
use core::ptr::NonNull;
use cranelift_entity::PrimaryMap;
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
    engine: Engine,
    translated_module: TranslatedModule,
    required_features: WasmFeatures,
    vmshape: VMShape,
    code: Arc<CodeObject>,
    type_collection: RuntimeTypeCollection,
    function_info: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
}

impl Module {
    pub fn from_bytes(
        engine: &Engine,
        store: &mut StoreOpaque,
        validator: &mut Validator,
        bytes: &[u8],
    ) -> crate::Result<Self> {
        todo!()
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
    ) -> cranelift_entity::Iter<DefinedFuncIndex, CompiledFunctionInfo> {
        self.0.function_info.iter()
    }
    pub(super) fn array_to_wasm_trampoline(
        &self,
        index: DefinedFuncIndex,
    ) -> Option<NonNull<VMArrayCallFunction>> {
        let loc = self.0.function_info[index].array_to_wasm_trampoline?;
        let ptr = NonNull::new(self.code().resolve_function_loc(loc) as *mut VMArrayCallFunction)
            .unwrap();
        Some(ptr)
    }

    pub(super) fn function(&self, index: DefinedFuncIndex) -> NonNull<VMWasmCallFunction> {
        let loc = self.0.function_info[index].wasm_func_loc;
        NonNull::new(self.code().resolve_function_loc(loc) as *mut VMWasmCallFunction).unwrap()
    }

    pub(super) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn new_stub(engine: Engine) -> Self {
        let translated_module = TranslatedModule::default();
        Self(Arc::new(ModuleInner {
            engine: engine.clone(),
            vmshape: VMShape::for_module(u8_size_of::<usize>(), &translated_module),
            translated_module,
            required_features: WasmFeatures::default(),
            code: Arc::new(CodeObject::empty()),
            type_collection: RuntimeTypeCollection::empty(engine),
            function_info: PrimaryMap::default(),
        }))
    }
}
