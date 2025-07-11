// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::indices::{EntityIndex, VMSharedTypeIndex};
use crate::loom::sync::Arc;
use crate::type_registry::RuntimeTypeCollection;
use crate::wasm::{Import, ModuleParser, TranslatedModule};
use crate::Engine;
use core::ops::Deref;
use wasmparser::{Validator, WasmFeatures};

#[derive(Debug, Clone)]
pub struct Module(Arc<ModuleInner>);

#[derive(Debug)]
struct ModuleInner {
    engine: Engine,
    translated: TranslatedModule,
    required_features: WasmFeatures,
    type_collection: RuntimeTypeCollection,
}

// ===== impl Module =====

impl Module {
    pub fn from_bytes(
        engine: Engine,
        validator: &mut Validator,
        bytes: &[u8],
    ) -> crate::Result<Self> {
        let (translation, types) = ModuleParser::new(validator).translate(bytes)?;

        let inner = ModuleInner {
            // vmshape: VMShape::for_module(u8_size_of::<usize>(), &translation.module),
            type_collection: engine.type_registry().register_module_types(&engine, types),
            translated: translation.module,
            required_features: translation.required_features,
            engine,
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

    pub(crate) fn type_ids(&self) -> &[VMSharedTypeIndex] {
        self.0.type_collection.type_map().values().as_slice()
    }
}