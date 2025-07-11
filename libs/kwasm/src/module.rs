// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::vec::Vec;
use core::mem;
use core::ptr::NonNull;

use wasmparser::{Validator, WasmFeatures};

use crate::Engine;
use crate::compile::{CompileInputs, CompiledFunctionInfo};
use crate::indices::{DefinedFuncIndex, EntityIndex, VMSharedTypeIndex};
use crate::loom::sync::Arc;
use crate::type_registry::RuntimeTypeCollection;
use crate::utils::u8_size_of;
use crate::vm::{CodeObject, Mmap, VMArrayCallNative, VMContextShape, VMWasmCallFunction};
use crate::wasm::{Import, ModuleParser, TranslatedModule, WasmparserTypeConverter};

#[derive(Debug, Clone)]
pub struct Module(Arc<ModuleInner>);

#[derive(Debug)]
struct ModuleInner {
    engine: Engine,
    translated: TranslatedModule,
    required_features: WasmFeatures,
    type_collection: RuntimeTypeCollection,
    vmctx_shape: VMContextShape,
    code: Arc<CodeObject>,
}

// ===== impl Module =====

impl Module {
    /// # Errors
    ///
    /// TODO
    pub fn from_bytes(
        engine: Engine,
        validator: &mut Validator,
        do_mmap: impl FnOnce(Vec<u8>) -> crate::Result<Mmap>,
        bytes: &[u8],
    ) -> crate::Result<Self> {
        let (mut translation, types) = ModuleParser::new(validator).translate(bytes)?;

        tracing::debug!("Gathering compile inputs...");
        let function_body_data = mem::take(&mut translation.function_bodies);
        let ty_cvt = WasmparserTypeConverter::new(&types, &translation.module);
        let inputs = CompileInputs::from_module(&translation, &types, function_body_data, &ty_cvt);

        tracing::debug!("Compiling inputs...");
        let unlinked_outputs = inputs.compile(engine.compiler())?;

        let type_collection = engine.type_registry().register_module_types(&engine, types);

        let mut code = unlinked_outputs.link_and_finish(&engine, &translation.module, |code| {
            tracing::debug!("Allocating new memory map...");

            let mut mmap = do_mmap(code)?;

            if let Some(name) = translation.module.name.clone() {
                mmap = mmap.named(name);
            }

            Ok(mmap)
        })?;

        code.publish()?;
        let code = Arc::new(code);

        // // register this code memory with the trap handler, so we can correctly unwind from traps
        // register_code(&code);

        let inner = ModuleInner {
            vmctx_shape: VMContextShape::for_module(u8_size_of::<usize>(), &translation.module),
            translated: translation.module,
            required_features: translation.required_features,
            engine,
            code,
            type_collection,
        };

        Ok(Self(Arc::new(inner)))
    }

    /// Returns the modules name if present.
    pub fn name(&self) -> Option<&str> {
        self.0.translated.name.as_deref()
    }

    /// Returns the modules imports.
    pub fn imports(&self) -> impl ExactSizeIterator<Item = &Import> {
        self.0.translated.imports.iter()
    }

    /// Returns the modules exports.
    pub fn exports(&self) -> impl ExactSizeIterator<Item = (&str, EntityIndex)> + '_ {
        self.0
            .translated
            .exports
            .iter()
            .map(|(name, index)| (name.as_str(), *index))
    }

    /// WebAssembly features that are required to instantiate this [`Module`].
    pub fn required_features(&self) -> WasmFeatures {
        self.0.required_features
    }

    /// The [`Engine`] that this [`Module`] has been instantiated within.
    pub(crate) fn engine(&self) -> &Engine {
        &self.0.engine
    }

    pub(crate) fn translated(&self) -> &TranslatedModule {
        &self.0.translated
    }

    pub(crate) fn code(&self) -> &Arc<CodeObject> {
        &self.0.code
    }

    pub(crate) fn vmctx_shape(&self) -> &VMContextShape {
        &self.0.vmctx_shape
    }

    pub(crate) fn type_ids(&self) -> &[VMSharedTypeIndex] {
        self.0.type_collection.type_map().values().as_slice()
    }

    pub(crate) fn functions(
        &self,
    ) -> cranelift_entity::Iter<'_, DefinedFuncIndex, CompiledFunctionInfo> {
        self.0.code.function_info().iter()
    }

    pub(crate) fn array_to_wasm_trampoline(
        &self,
        index: DefinedFuncIndex,
    ) -> Option<VMArrayCallNative> {
        let loc = self.0.code.function_info()[index].array_to_wasm_trampoline?;
        let raw = self.code().resolve_function_loc(loc);
        // Safety: TODO
        Some(unsafe { mem::transmute::<usize, VMArrayCallNative>(raw) })
    }

    /// Return the address, in memory, of the trampoline that allows Wasm to
    /// call a array function of the given signature.
    pub(crate) fn wasm_to_array_trampoline(
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

    pub(crate) fn function(&self, index: DefinedFuncIndex) -> NonNull<VMWasmCallFunction> {
        let loc = self.0.code.function_info()[index].wasm_func_loc;
        NonNull::new(self.code().resolve_function_loc(loc) as *mut VMWasmCallFunction).unwrap()
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn new_stub(engine: Engine) -> Self {
        let translated = TranslatedModule::default();
        Self(Arc::new(ModuleInner {
            // name: "<Stub>".to_string(),
            engine: engine.clone(),
            vmctx_shape: VMContextShape::for_module(u8_size_of::<usize>(), &translated),
            translated,
            required_features: WasmFeatures::default(),
            code: Arc::new(CodeObject::empty()),
            type_collection: RuntimeTypeCollection::empty(engine),
        }))
    }
}
