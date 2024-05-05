mod compiled_function;
mod compiled_module;
mod compiler;

use crate::runtime::compile::compiled_function::{CompiledFunction, RelocationTarget};
use crate::runtime::engine::Engine;
use crate::runtime::translate::ModuleEnvironment;
use crate::runtime::translate::ModuleTranslation;
use crate::runtime::translate::{FunctionBodyInput, Module};
use crate::runtime::CompileError;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::fmt;
use core::fmt::Formatter;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_wasm::wasmparser::{Parser, Validator, WasmFeatures};
use cranelift_wasm::{DefinedFuncIndex, ModuleInternedTypeIndex, StaticModuleIndex, WasmSubType};

use crate::runtime::builtins::BuiltinFunctionIndex;
use crate::runtime::compile::compiled_module::{CompiledFunctionInfo, ModuleTextBuilder};
pub use compiler::Compiler;

pub fn compile_module<'wasm>(engine: &Engine, wasm: &'wasm [u8]) {
    // 2. Setup parsing & translation state
    let features = WasmFeatures::default();
    let mut validator = Validator::new_with_features(features);
    let parser = Parser::new(0);
    let module_env = ModuleEnvironment::new(&mut validator);

    // 3. Perform WASM -> Cranelift IR translation
    let translation = module_env.translate(parser, wasm).unwrap();
    let ModuleTranslation {
        module,
        function_body_inputs,
        types,
    } = translation;

    // 4. collect all the necessary context and gather the functions that need compiling
    let compile_inputs = CompileInputs::from_module(&module, &types, function_body_inputs);

    // 5. compile functions to machine code
    let unlinked_compile_outputs = compile_inputs.compile(&engine, &module).unwrap();

    let mut text_builder = ModuleTextBuilder::new(
        engine
            .target_isa()
            .text_section_builder(unlinked_compile_outputs.num_funcs()),
    );

    // 6. link functions & resolve relocations
    let compiled_module = unlinked_compile_outputs.link_and_append(module, &mut text_builder);

    todo!()
}

type CompileInput<'a> =
    Box<dyn FnOnce(&Compiler) -> Result<CompileOutput, CompileError> + Send + 'a>;

pub struct CompileInputs<'a> {
    inputs: Vec<CompileInput<'a>>,
}

impl<'a> CompileInputs<'a> {
    pub fn from_module(
        module: &'a Module,
        types: &'a PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
        function_body_inputs: PrimaryMap<DefinedFuncIndex, FunctionBodyInput<'a>>,
    ) -> Self {
        let mut inputs: Vec<CompileInput> = Vec::new();
        let mut num_trampolines = 0;

        for (def_func_index, body_input) in function_body_inputs {
            inputs.push(Box::new(move |compiler| {
                let function =
                    compiler.compile_function(&module, &types, def_func_index, body_input)?;

                Ok(CompileOutput {
                    key: CompileKey::wasm_function(StaticModuleIndex::from_u32(0), def_func_index),
                    function,
                })
            }));

            // Compile a native->wasm trampoline for every function that *could theoretically* be
            // called by native code.
            let func_index = module.func_index(def_func_index);
            if module.functions[func_index].is_escaping() {
                num_trampolines += 1;
                inputs.push(Box::new(move |compiler| {
                    let function = compiler.compile_native_to_wasm_trampoline(
                        &module,
                        &types,
                        def_func_index,
                    )?;

                    Ok(CompileOutput {
                        key: CompileKey::native_to_wasm_trampoline(
                            StaticModuleIndex::from_u32(0),
                            def_func_index,
                        ),
                        function,
                    })
                }));
            }
        }

        log::debug!("Number of native to WASM trampolines to build: {num_trampolines}",);

        // TODO collect wasm->native trampolines

        Self { inputs }
    }

    pub fn compile(
        self,
        engine: &Engine,
        _module: &'a Module,
    ) -> Result<UnlinkedCompileOutputs, CompileError> {
        let mut indices = BTreeMap::new();
        let mut outputs: BTreeMap<u32, BTreeMap<CompileKey, CompileOutput>> = BTreeMap::new();

        for (idx, f) in self.inputs.into_iter().enumerate() {
            let output = f(engine.compiler())?;
            indices.insert(output.key, idx);

            outputs
                .entry(output.key.kind())
                .or_default()
                .insert(output.key, output);
        }

        // let functions = outputs.get(&CompileOutputKind::Function).unwrap();
        // let mut builtins = Vec::new();
        // compile_required_builtins(engine, module, functions, &mut builtins)?;
        // outputs.insert(CompileOutputKind::WasmToBuiltinTrampoline, builtins);

        Ok(UnlinkedCompileOutputs { indices, outputs })
    }
}

fn compile_required_builtins(
    engine: &Engine,
    module: &Module,
    func_outputs: &[CompileOutput],
    builtin_outputs: &mut Vec<CompileOutput>,
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

        builtin_outputs.push(CompileOutput {
            key: CompileKey::wasm_to_builtin_trampoline(builtin_index),
            function,
        });
    }

    Ok(())
}

#[derive(Debug)]
struct CompileOutput {
    key: CompileKey,
    function: CompiledFunction,
}

/// A sortable, comparable key for a compilation output.
///
/// Two `u32`s to align with `cranelift_codegen::ir::UserExternalName`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct CompileKey {
    // The namespace field is bitpacked like:
    //
    //     [ kind:i3 module:i29 ]
    namespace: u32,

    index: u32,
}

impl CompileKey {
    const KIND_BITS: u32 = 3;
    const KIND_OFFSET: u32 = 32 - Self::KIND_BITS;
    const KIND_MASK: u32 = ((1 << Self::KIND_BITS) - 1) << Self::KIND_OFFSET;

    fn kind(&self) -> u32 {
        self.namespace & Self::KIND_MASK
    }

    fn module(&self) -> StaticModuleIndex {
        StaticModuleIndex::from_u32(self.namespace & !Self::KIND_MASK)
    }

    const WASM_FUNCTION_KIND: u32 = Self::new_kind(0);
    const ARRAY_TO_WASM_TRAMPOLINE_KIND: u32 = Self::new_kind(1);
    const NATIVE_TO_WASM_TRAMPOLINE_KIND: u32 = Self::new_kind(2);
    const WASM_TO_NATIVE_TRAMPOLINE_KIND: u32 = Self::new_kind(3);
    const WASM_TO_BUILTIN_TRAMPOLINE_KIND: u32 = Self::new_kind(4);

    const fn new_kind(kind: u32) -> u32 {
        assert!(kind < (1 << Self::KIND_BITS));
        kind << Self::KIND_OFFSET
    }

    // NB: more kinds in the other `impl` block.

    fn wasm_function(module: StaticModuleIndex, index: DefinedFuncIndex) -> Self {
        debug_assert_eq!(module.as_u32() & Self::KIND_MASK, 0);
        Self {
            namespace: Self::WASM_FUNCTION_KIND | module.as_u32(),
            index: index.as_u32(),
        }
    }

    fn native_to_wasm_trampoline(module: StaticModuleIndex, index: DefinedFuncIndex) -> Self {
        debug_assert_eq!(module.as_u32() & Self::KIND_MASK, 0);
        Self {
            namespace: Self::NATIVE_TO_WASM_TRAMPOLINE_KIND | module.as_u32(),
            index: index.as_u32(),
        }
    }

    fn wasm_to_builtin_trampoline(index: BuiltinFunctionIndex) -> Self {
        Self {
            namespace: Self::WASM_TO_BUILTIN_TRAMPOLINE_KIND,
            index: index.index(),
        }
    }

    // fn array_to_wasm_trampoline(module: StaticModuleIndex, index: DefinedFuncIndex) -> Self {
    //     debug_assert_eq!(module.as_u32() & Self::KIND_MASK, 0);
    //     Self {
    //         namespace: Self::ARRAY_TO_WASM_TRAMPOLINE_KIND | module.as_u32(),
    //         index: index.as_u32(),
    //     }
    // }

    // fn wasm_to_native_trampoline(index: ModuleInternedTypeIndex) -> Self {
    //     Self {
    //         namespace: Self::WASM_TO_NATIVE_TRAMPOLINE_KIND,
    //         index: index.as_u32(),
    //     }
    // }
}

#[derive(Debug)]
struct UnlinkedCompileOutputs {
    indices: BTreeMap<CompileKey, usize>,
    outputs: BTreeMap<u32, BTreeMap<CompileKey, CompileOutput>>,
}

impl UnlinkedCompileOutputs {
    pub fn code_size_hint(&self) -> usize {
        self.iter_flattened().fold(0, |acc, output| {
            acc + output.function.buffer.total_size() as usize
        })
    }

    pub fn num_funcs(&self) -> usize {
        self.outputs.iter().fold(0, |acc, (_, vec)| acc + vec.len())
    }

    pub fn iter_flattened(&self) -> impl Iterator<Item = &CompileOutput> + '_ {
        self.outputs.values().map(|inner| inner.values()).flatten()
    }

    pub fn link_and_append<'wasm>(
        self,
        module: Module<'wasm>,
        text_builder: &mut ModuleTextBuilder,
    ) -> PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> {
        text_builder.append_funcs(self.iter_flattened(), |callee| match callee {
            RelocationTarget::Wasm(callee_index) => {
                let def_func_index = module.defined_func_index(callee_index).unwrap();
                self.indices
                    [&CompileKey::wasm_function(StaticModuleIndex::from_u32(0), def_func_index)]
            }
            RelocationTarget::Builtin(builtin_index) => {
                self.indices[&CompileKey::wasm_to_builtin_trampoline(builtin_index)]
            }
        })
    }
}
