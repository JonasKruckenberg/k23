use crate::vm::AddressSpace;
use crate::wasm::compile::{CompileInputs, CompiledFunctionInfo};
use crate::wasm::indices::{DefinedFuncIndex, EntityIndex, VMSharedTypeIndex};
use crate::wasm::runtime::{code_registry, CodeMemory};
use crate::wasm::runtime::{MmapVec, VMOffsets};
use crate::wasm::translate::{Import, TranslatedModule};
use crate::wasm::type_registry::RuntimeTypeCollection;
use crate::wasm::{Engine, ModuleTranslator};
use alloc::sync::Arc;
use core::mem;
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
    translated: TranslatedModule,
    offsets: VMOffsets,
    code: Arc<CodeMemory>,
    type_collection: RuntimeTypeCollection,
    function_info: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
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
    #[expect(tail_expr_drop_order, reason = "")]
    pub fn from_bytes(
        engine: &Engine,
        aspace: &mut AddressSpace,
        validator: &mut Validator,
        bytes: &[u8],
    ) -> crate::wasm::Result<Self> {
        let (mut translation, types) = ModuleTranslator::new(validator).translate(bytes)?;

        log::debug!("Gathering compile inputs...");
        let function_body_data = mem::take(&mut translation.function_bodies);
        let inputs = CompileInputs::from_module(&translation, &types, function_body_data);

        log::debug!("Compiling inputs...");
        let unlinked_outputs = inputs.compile(engine.compiler())?;

        log::debug!("Applying static relocations...");
        let (code, function_info, (trap_offsets, traps)) =
            unlinked_outputs.link_and_finish(engine, &translation.module);

        let type_collection = engine.type_registry().register_module_types(engine, types);

        log::debug!("Allocating new memory map...");
        let vec = MmapVec::from_slice(aspace, &code)?;
        let mut code = CodeMemory::new(vec, trap_offsets, traps);
        code.publish(aspace)?;
        let code = Arc::new(code);

        // register this code memory with the trap handler, so we can correctly unwind from traps
        code_registry::register_code(&code);

        Ok(Self(Arc::new(ModuleInner {
            offsets: VMOffsets::for_module(
                engine.compiler().triple().pointer_width().unwrap().bytes(),
                &translation.module,
            ),
            translated: translation.module,
            function_info,
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

    pub(crate) fn get_export(&self, name: &str) -> Option<EntityIndex> {
        self.0.translated.exports.get(name).copied()
    }

    pub(crate) fn translated(&self) -> &TranslatedModule {
        &self.0.translated
    }
    pub(crate) fn offsets(&self) -> &VMOffsets {
        &self.0.offsets
    }
    pub(crate) fn code(&self) -> &CodeMemory {
        &self.0.code
    }
    pub(crate) fn type_collection(&self) -> &RuntimeTypeCollection {
        &self.0.type_collection
    }
    pub(crate) fn type_ids(&self) -> &[VMSharedTypeIndex] {
        self.0.type_collection.type_map().values().as_slice()
    }
    pub(crate) fn function_info(&self) -> &PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> {
        &self.0.function_info
    }
}
