mod compiled_func;
mod compiler;
mod obj_builder;

pub use compiler::Compiler;
pub use obj_builder::{
    ELFOSABI_K23, ELF_K23_BTI, ELF_K23_ENGINE, ELF_K23_INFO, ELF_K23_TRAPS, ELF_TEXT,
    ELF_WASM_DATA, ELF_WASM_DWARF, ELF_WASM_NAMES,
};

use crate::runtime::builtins::BuiltinFunctionIndex;
use crate::runtime::compile::compiled_func::{CompiledFunction, RelocationTarget};
use crate::runtime::compile::obj_builder::ObjectBuilder;
use crate::runtime::errors::CompileError;
use crate::runtime::translate::ModuleEnvironment;
use crate::runtime::translate::{TranslatedModule, Translation};
use crate::runtime::Engine;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use cranelift_entity::PrimaryMap;
use cranelift_wasm::wasmparser::{
    FuncToValidate, FunctionBody, Parser, Validator, ValidatorResources,
};
use cranelift_wasm::{DefinedFuncIndex, ModuleInternedTypeIndex, StaticModuleIndex, WasmSubType};
use object::write::WritableBuffer;

pub fn compile_module<'wasm, T: WritableBuffer>(
    engine: &Engine,
    wasm: &'wasm [u8],
    output_buffer: &mut T,
) -> Result<CompiledModuleInfo<'wasm>, CompileError> {
    let mut validator = Validator::new_with_features(engine.wasm_features());
    let parser = Parser::new(0);
    let module_env = ModuleEnvironment::new(&mut validator);

    // Perform WASM -> Cranelift IR translation
    log::trace!("Translating module to Cranelift IR...");
    let translation = module_env.translate(parser, wasm)?;
    let Translation {
        module,
        func_compile_inputs,
        types,
    } = translation;

    engine.assert_compatible(&module);

    // collect all the necessary context and gather the functions that need compiling
    let compile_inputs = CompileInputs::from_module(&module, &types, func_compile_inputs);

    // compile functions to machine code
    log::trace!("Compiling functions to machine code...");
    let unlinked_compile_outputs = compile_inputs.compile(engine, &module)?;

    log::trace!("Setting up intermediate code object...");
    let mut obj_builder = ObjectBuilder::new(engine.compiler().create_intermediate_code_object());

    log::trace!("Appending info to intermediate code object...");
    obj_builder.append_engine_info(engine);
    obj_builder.append_debug_info(&module.debug_info);

    log::trace!("Appending compiled functions to intermediate code object...");
    let info = unlinked_compile_outputs.link_append_and_finish(
        engine,
        module,
        types,
        obj_builder,
        output_buffer,
    );

    Ok(info)
}

pub struct FuncCompileInput<'wasm> {
    pub body: FunctionBody<'wasm>,
    pub validator: FuncToValidate<ValidatorResources>,
}

#[derive(Debug)]
pub struct CompiledModuleInfo<'wasm> {
    pub module: TranslatedModule<'wasm>,
    pub funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
    pub types: PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
}

#[derive(Debug)]
pub struct CompiledFunctionInfo {
    /// The [`FunctionLoc`] indicating the location of this function in the text
    /// section of the compilation artifact.
    pub wasm_func_loc: FunctionLoc,
    /// A trampoline for native callers (e.g. `Func::wrap`) calling into this function (if needed).
    pub native_to_wasm_trampoline: Option<FunctionLoc>,
}

/// Description of where a function is located in the text section of a
/// compiled image.
#[derive(Debug, Copy, Clone)]
pub struct FunctionLoc {
    /// The byte offset from the start of the text section where this
    /// function starts.
    pub start: u32,
    /// The byte length of this function's function body.
    pub length: u32,
}

type CompileInput<'a> =
    Box<dyn FnOnce(&Compiler) -> Result<CompileOutput, CompileError> + Send + 'a>;

pub struct CompileInputs<'a>(Vec<CompileInput<'a>>);

impl<'a> CompileInputs<'a> {
    /// Gather all functions that need compilation - including trampolines.
    pub fn from_module(
        module: &'a TranslatedModule,
        types: &'a PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
        function_body_inputs: PrimaryMap<DefinedFuncIndex, FuncCompileInput<'a>>,
    ) -> Self {
        let mut inputs: Vec<CompileInput> = Vec::new();
        // let mut num_trampolines = 0;
        // We only ever compile one module at a time
        let module_index = StaticModuleIndex::from_u32(0);

        for (def_func_index, body_input) in function_body_inputs {
            inputs.push(Box::new(move |compiler| {
                let function =
                    compiler.compile_function(module, types, def_func_index, body_input)?;

                Ok(CompileOutput {
                    key: CompileKey::wasm_function(module_index, def_func_index),
                    function,
                    symbol: format!(
                        "wasm[{}]::function[{}]",
                        module_index.as_u32(),
                        def_func_index.as_u32()
                    ),
                })
            }));

            // Compile a native->wasm trampoline for every function that *could theoretically* be
            // called by native code.
            // let func_index = module.function_index(def_func_index);
            // if module.functions[func_index].is_escaping() {
            //     num_trampolines += 1;
            //     inputs.push(Box::new(move |compiler| {
            //         let function = compiler.compile_native_to_wasm_trampoline(
            //             &module,
            //             &types,
            //             def_func_index,
            //         )?;
            //
            //         Ok(CompileOutput {
            //             key: CompileKey::native_to_wasm_trampoline(module_index, def_func_index),
            //             function,
            //             symbol: format!(
            //                 "wasm[{}]::native_to_wasm_trampoline[{}]",
            //                 module_index.as_u32(),
            //                 func_index.as_u32()
            //             ),
            //         })
            //     }));
            // }
        }

        // log::debug!("Number of native to WASM trampolines to build: {num_trampolines}",);

        // TODO collect wasm->native trampolines

        Self(inputs)
    }

    /// Feed the collected inputs through the compiler, producing [`UnlinkedCompileOutputs`] which holds
    /// the resulting artifacts.
    pub fn compile(
        self,
        engine: &Engine,
        module: &TranslatedModule,
    ) -> Result<UnlinkedCompileOutputs, CompileError> {
        let mut indices = BTreeMap::new();
        let mut outputs: BTreeMap<u32, BTreeMap<CompileKey, CompileOutput>> = BTreeMap::new();

        for (idx, f) in self.0.into_iter().enumerate() {
            let output = f(engine.compiler())?;
            indices.insert(output.key, idx);

            outputs
                .entry(output.key.kind())
                .or_default()
                .insert(output.key, output);
        }

        let mut unlinked_compile_outputs = UnlinkedCompileOutputs { indices, outputs };
        let flattened: Vec<_> = unlinked_compile_outputs
            .outputs
            .values()
            .flat_map(|inner| inner.values())
            .collect();

        let mut builtins = BTreeMap::new();

        compile_required_builtins(engine, module, flattened.into_iter(), &mut builtins)?;

        unlinked_compile_outputs
            .outputs
            .insert(CompileKey::WASM_TO_BUILTIN_TRAMPOLINE_KIND, builtins);

        Ok(unlinked_compile_outputs)
    }
}

/// Compile WASM to builtin trampolines for builtins referenced by the already compiled functions.
fn compile_required_builtins<'a>(
    engine: &Engine,
    module: &TranslatedModule,
    func_outputs: impl Iterator<Item = &'a CompileOutput>,
    builtin_outputs: &mut BTreeMap<CompileKey, CompileOutput>,
) -> Result<(), CompileError> {
    let mut builtins = BTreeSet::new();

    for out in func_outputs {
        for reloc in out.function.relocations() {
            if let RelocationTarget::Builtin(builtin_index) = reloc.target {
                builtins.insert(builtin_index);
            }
        }
    }

    log::debug!(
        "Number of WASM to builtin trampolines to build: {}",
        builtins.len()
    );

    for builtin_index in builtins {
        let function = engine
            .compiler()
            .compile_wasm_to_builtin_trampoline(module, builtin_index)?;

        let key = CompileKey::wasm_to_builtin_trampoline(builtin_index);
        builtin_outputs.insert(
            key,
            CompileOutput {
                key,
                function,
                symbol: format!("wasm_to_builtin_trampoline[{}]", builtin_index.as_u32()),
            },
        );
    }

    Ok(())
}

pub struct UnlinkedCompileOutputs {
    indices: BTreeMap<CompileKey, usize>,
    outputs: BTreeMap<u32, BTreeMap<CompileKey, CompileOutput>>,
}

#[derive(Debug)]
pub struct CompileOutput {
    pub key: CompileKey,
    pub function: CompiledFunction,
    pub symbol: String,
}

impl UnlinkedCompileOutputs {
    /// Append the compiled functions to the given object resolving any relocations in the process.
    ///
    /// This is the final step if compilation.
    pub fn link_append_and_finish<'wasm, T: WritableBuffer>(
        mut self,
        engine: &Engine,
        module: TranslatedModule<'wasm>,
        types: PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
        mut obj_builder: ObjectBuilder,
        output_buffer: &mut T,
    ) -> CompiledModuleInfo<'wasm> {
        let flattened: Vec<_> = self
            .outputs
            .values()
            .flat_map(|inner| inner.values())
            .collect();

        let text_builder = engine
            .compiler()
            .target_isa()
            .text_section_builder(flattened.len());

        let mut text_builder = obj_builder.text_builder(text_builder);

        let symbol_ids_and_locs =
            text_builder.push_funcs(flattened.into_iter(), |callee| match callee {
                RelocationTarget::Wasm(callee_index) => {
                    let def_func_index = module.defined_function_index(callee_index).unwrap();
                    self.indices
                        [&CompileKey::wasm_function(StaticModuleIndex::from_u32(0), def_func_index)]
                }
                RelocationTarget::Builtin(builtin_index) => {
                    self.indices[&CompileKey::wasm_to_builtin_trampoline(builtin_index)]
                }
            });

        text_builder.finish();

        let wasm_functions = self
            .outputs
            .remove(&CompileKey::WASM_FUNCTION_KIND)
            .unwrap_or_default()
            .into_iter();

        // let mut native_to_wasm_trampolines = self
        //     .outputs
        //     .remove(&CompileKey::NATIVE_TO_WASM_TRAMPOLINE_KIND)
        //     .unwrap_or_default();

        let funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> = wasm_functions
            .map(|(key, _)| {
                let wasm_func_index = self.indices[&key];
                let (_, wasm_func_loc) = symbol_ids_and_locs[wasm_func_index];

                // let native_to_wasm_trampoline_key = CompileKey::native_to_wasm_trampoline(
                //     key.module(),
                //     DefinedFuncIndex::from_u32(key.index),
                // );
                // let native_to_wasm_trampoline = native_to_wasm_trampolines
                //     .remove(&native_to_wasm_trampoline_key)
                //     .map(|output| symbol_ids_and_locs[self.indices[&output.key]].1);

                CompiledFunctionInfo {
                    wasm_func_loc,
                    native_to_wasm_trampoline: None,
                }
            })
            .collect();

        // If configured attempt to use static memory initialization which
        // can either at runtime be implemented as a single memcpy to
        // initialize memory or otherwise enabling virtual-memory-tricks
        // such as mmap'ing from a file to get copy-on-write.
        // let max_always_allowed = kconfig::PAGE_SIZE * 16; // TODO
        // module.try_static_init(kconfig::PAGE_SIZE, max_always_allowed);

        // Attempt to convert table initializer segments to
        // FuncTable representation where possible, to enable
        // table lazy init.
        // module.try_func_table_init();

        obj_builder.finish(output_buffer).unwrap();

        CompiledModuleInfo {
            module,
            funcs,
            types,
        }
    }
}

/// A sortable, comparable key for a compilation output.
/// This is used to sort by compilation output kind and bucket results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompileKey {
    // The namespace field is bitpacked like:
    //
    //     [ kind:i3 module:i29 ]
    namespace: u32,
    pub index: u32,
}

impl CompileKey {
    const KIND_BITS: u32 = 3;
    const KIND_OFFSET: u32 = 32 - Self::KIND_BITS;
    const KIND_MASK: u32 = ((1 << Self::KIND_BITS) - 1) << Self::KIND_OFFSET;

    pub fn kind(self) -> u32 {
        self.namespace & Self::KIND_MASK
    }

    pub fn module(self) -> StaticModuleIndex {
        StaticModuleIndex::from_u32(self.namespace & !Self::KIND_MASK)
    }

    pub const WASM_FUNCTION_KIND: u32 = Self::new_kind(0);
    // const ARRAY_TO_WASM_TRAMPOLINE_KIND: u32 = Self::new_kind(1);
    // const NATIVE_TO_WASM_TRAMPOLINE_KIND: u32 = Self::new_kind(2);
    // const WASM_TO_NATIVE_TRAMPOLINE_KIND: u32 = Self::new_kind(3);
    pub const WASM_TO_BUILTIN_TRAMPOLINE_KIND: u32 = Self::new_kind(4);

    const fn new_kind(kind: u32) -> u32 {
        assert!(kind < (1 << Self::KIND_BITS));
        kind << Self::KIND_OFFSET
    }

    pub fn wasm_function(module: StaticModuleIndex, index: DefinedFuncIndex) -> Self {
        debug_assert_eq!(module.as_u32() & Self::KIND_MASK, 0);
        Self {
            namespace: Self::WASM_FUNCTION_KIND | module.as_u32(),
            index: index.as_u32(),
        }
    }

    pub fn wasm_to_builtin_trampoline(index: BuiltinFunctionIndex) -> Self {
        Self {
            namespace: Self::WASM_TO_BUILTIN_TRAMPOLINE_KIND,
            index: index.as_u32(),
        }
    }

    // fn native_to_wasm_trampoline(module: StaticModuleIndex, index: DefinedFuncIndex) -> Self {
    //     debug_assert_eq!(module.as_u32() & Self::KIND_MASK, 0);
    //     Self {
    //         namespace: Self::NATIVE_TO_WASM_TRAMPOLINE_KIND | module.as_u32(),
    //         index: index.as_u32(),
    //     }
    // }

    // fn array_to_wasm_trampoline(module: StaticModuleIndex, index: DefinedFuncIndex) -> Self {
    //     debug_assert_eq!(module.as_u32() & Self::KIND_MASK, 0);
    //     Self {
    //         namespace: Self::ARRAY_TO_WASM_TRAMPOLINE_KIND | module.as_u32(),
    //         index: index.as_u32(),
    //     }
    // }
    //
    // fn wasm_to_native_trampoline(index: ModuleInternedTypeIndex) -> Self {
    //     Self {
    //         namespace: Self::WASM_TO_NATIVE_TRAMPOLINE_KIND,
    //         index: index.as_u32(),
    //     }
    // }
}
