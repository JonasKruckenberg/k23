use crate::vm::AddressSpace;
use crate::wasm::compile::{CompileInputs, CompiledFunctionInfo};
use crate::wasm::indices::{DefinedFuncIndex, EntityIndex, VMSharedTypeIndex};
use crate::wasm::runtime::{code_registry, CodeMemory, VMWasmCallFunction};
use crate::wasm::runtime::{MmapVec, VMOffsets};
use crate::wasm::translate::{Import, TranslatedModule};
use crate::wasm::type_registry::RuntimeTypeCollection;
use crate::wasm::{Engine, ModuleTranslator, Store};
use alloc::sync::Arc;
use core::any::Any;
use core::mem;
use core::ptr::NonNull;
use cranelift_entity::PrimaryMap;
use wasmparser::Validator;

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
    translated: TranslatedModule,
    offsets: VMOffsets,
    code: Arc<CodeMemory>,
    type_collection: RuntimeTypeCollection,
}

impl Module {
    // /// Creates a new module from the given WebAssembly text format.
    // ///
    // /// This will parse, translate and compile the module and is the first step in Wasm execution.
    // ///
    // /// # Errors
    // ///
    // /// Returns an error if the WebAssembly text file is malformed, or compilation fails.
    // pub fn from_str(engine: &Engine, validator: &mut Validator, str: &str) -> crate::wasm::Result<Self> {
    //     let bytes = wat::parse_str(str)?;
    //     Self::from_bytes(engine, validator, &bytes)
    // }

    /// Creates a new module from the given WebAssembly bytes.
    ///
    /// This will parse, translate and compile the module and is the first step in Wasm execution.
    ///
    /// # Errors
    ///
    /// Returns an error if the WebAssembly module is malformed, or compilation fails.
    ///
    /// # Panics
    ///
    /// TODO
    pub fn from_bytes(
        engine: &Engine,
        store: &mut Store,
        validator: &mut Validator,
        bytes: &[u8],
    ) -> crate::wasm::Result<Self> {
        let (mut translation, types) = ModuleTranslator::new(validator).translate(bytes)?;

        tracing::debug!("Gathering compile inputs...");
        let function_body_data = mem::take(&mut translation.function_bodies);
        let inputs = CompileInputs::from_module(&translation, &types, function_body_data);

        tracing::debug!("Compiling inputs...");
        let unlinked_outputs = inputs.compile(engine.compiler())?;

        tracing::debug!("Applying static relocations...");
        let code = {
            let mut aspace = store.alloc.0.lock();
            let mut code =
                unlinked_outputs.link_and_finish(engine, &translation.module, |code| {
                    tracing::debug!("Allocating new memory map...");

                    MmapVec::from_slice(&mut aspace, &code)
                })?;
            code.publish(&mut aspace)?;

            drop(aspace);
            Arc::new(code)
        };

        let type_collection = engine.type_registry().register_module_types(engine, types);

        // register this code memory with the trap handler, so we can correctly unwind from traps
        code_registry::register_code(&code);

        Ok(Self(Arc::new(ModuleInner {
            engine: engine.clone(),
            offsets: VMOffsets::for_module(
                engine.compiler().triple().pointer_width().unwrap().bytes(),
                &translation.module,
            ),
            translated: translation.module,
            code,
            type_collection,
        })))
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

    /// Returns the modules name if present.
    pub fn name(&self) -> Option<&str> {
        self.0.translated.name.as_deref()
    }

    pub(super) fn get_export(&self, name: &str) -> Option<EntityIndex> {
        self.0.translated.exports.get(name).copied()
    }

    pub(super) fn translated(&self) -> &TranslatedModule {
        &self.0.translated
    }
    pub(super) fn offsets(&self) -> &VMOffsets {
        &self.0.offsets
    }
    pub(super) fn code(&self) -> &CodeMemory {
        &self.0.code
    }
    pub(super) fn type_collection(&self) -> &RuntimeTypeCollection {
        &self.0.type_collection
    }
    pub(super) fn type_ids(&self) -> &[VMSharedTypeIndex] {
        self.0.type_collection.type_map().values().as_slice()
    }
    pub(super) fn wasm_to_array_trampoline(
        &self,
        sig: VMSharedTypeIndex,
    ) -> Option<NonNull<VMWasmCallFunction>> {
        let trampoline_shared_ty = self.0.engine.type_registry().get_trampoline_type(sig);
        let trampoline_module_ty = self
            .0
            .type_collection
            .trampoline_type(trampoline_shared_ty)?;

        debug_assert!(
            self.0
                .engine
                .type_registry()
                .get_type(
                    &self.0.engine,
                    self.0
                        .type_collection
                        .lookup_shared_type(trampoline_module_ty)
                        .unwrap()
                )
                .unwrap()
                .unwrap_func()
                .is_trampoline_type()
        );

        let ptr = self
            .code()
            .wasm_to_host_trampoline(trampoline_module_ty)
            .cast_mut();

        Some(NonNull::new(ptr).unwrap())
    }
}
