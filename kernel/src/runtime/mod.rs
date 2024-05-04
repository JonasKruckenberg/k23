mod allocator;
mod builtins;
mod compile;
mod engine;
mod error;
mod store;
mod utils;
mod vmcontext;
mod wasm2ir;

use crate::runtime::compile::{CompiledFunction, Relocation, RelocationTarget};
use crate::runtime::wasm2ir::{FunctionBodyInput, Module};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt::{Debug, Formatter};
use core::mem::Discriminant;
use cranelift_codegen::entity::{EntityRef, PrimaryMap, SecondaryMap};
use cranelift_wasm::wasmparser::{Validator, WasmFeatures};
use cranelift_wasm::{DefinedFuncIndex, ModuleInternedTypeIndex, WasmSubType};

use crate::frame_alloc::with_frame_alloc;
use crate::kconfig;
use crate::runtime::store::{CodeMemory, Store};
pub use builtins::{BuiltinFunctionIndex, BuiltinFunctionSignatures, BuiltinFunctions};
pub use compile::Compiler;
pub use engine::Engine;
pub use error::CompileError;
pub use utils::FilePos;
pub use wasm2ir::{FuncEnvironment, ModuleEnvironment, ModuleTranslation};

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_WASM_BUILTIN: u32 = 1;

/// WebAssembly page sizes are defined to be 64KiB.
pub const WASM_PAGE_SIZE: u32 = 0x10000;

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;

/// Trap code used for debug assertions we emit in our JIT code.
pub const DEBUG_ASSERT_TRAP_CODE: u16 = u16::MAX;

pub fn setup_store(engine: &Engine) {
    // let asid = engine.next_address_space_id() as usize;
    // let root_table = with_mapper(0, |mapper, _| Ok(mapper.root_table().addr())).unwrap();

    // let store = Store::clone_from_kernel(0, root_table);
    //
    // store.activate::<kconfig::MEMORY_MODE>();
    //
    // log::trace!("{store:?}");
}

pub fn build_artifacts(engine: &Engine, wasm: &[u8]) -> Result<(), CompileError> {
    // CompiledModule, CompiledModuleInfo
    let features = WasmFeatures::default();
    let mut validator = Validator::new_with_features(features);
    let parser = cranelift_wasm::wasmparser::Parser::new(0);

    let mut module_env = ModuleEnvironment::new(&mut validator);
    let ModuleTranslation {
        module,
        function_body_inputs,
        types,
        ..
    } = module_env.translate(parser, wasm)?;

    log::debug!("module translation finished");

    // collect all the necessary context and gather the functions that need compiling
    let compile_inputs = CompileInputs::from_module(&module, &types, function_body_inputs);

    // compile functions to machine code
    let unlinked_compile_outputs = compile_inputs.compile(&engine, &module)?;

    log::debug!("{unlinked_compile_outputs:?}");

    // TODO pre_link reordering of functions for hot/cold optimization

    // let code_memory = CodeMemory::with_capacity(
    //     unlinked_compile_outputs.code_size_hint(),
    //     engine.guest_allocator(),
    // );
    //
    // log::debug!("allocated code memory {code_memory:?}");
    //
    // let compiled_module = unlinked_compile_outputs.link_and_append_code(module, code_memory);
    // log::debug!("{compiled_module:?}");

    Ok(())
}

type CompileInput<'a> =
    Box<dyn FnOnce(&Compiler) -> Result<CompileOutput, CompileError> + Send + 'a>;

struct CompileInputs<'a> {
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
                    kind: CompileOutputKind::Function,
                    function,
                    index: def_func_index.as_u32(),
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
                        kind: CompileOutputKind::NativeToWasmTrampoline,
                        function,
                        index: func_index.as_u32(),
                    })
                }));
            }
        }

        log::debug!("Number of native to WASM trampolines to build: {num_trampolines}",);

        // TODO collect wasm->native trampolines

        Self { inputs }
    }

    pub fn compile(
        mut self,
        engine: &Engine,
        module: &'a Module,
    ) -> Result<UnlinkedCompileOutputs, CompileError> {
        let mut outputs: BTreeMap<CompileOutputKind, Vec<CompileOutput>> = BTreeMap::new();

        for f in self.inputs {
            let output = f(engine.compiler())?;
            outputs.entry(output.kind).or_default().push(output);
        }

        let functions = outputs.get(&CompileOutputKind::Function).unwrap();
        let mut builtins = Vec::new();

        compile_required_builtins(engine, module, functions, &mut builtins)?;
        outputs.insert(CompileOutputKind::WasmToBuiltinTrampoline, builtins);

        Ok(UnlinkedCompileOutputs { outputs })
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
            kind: CompileOutputKind::WasmToBuiltinTrampoline,
            function,
            index: builtin_index.index(),
        });
    }

    Ok(())
}

#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq)]
enum CompileOutputKind {
    Function = 0,
    NativeToWasmTrampoline = 1,
    WasmToBuiltinTrampoline = 2,
}

#[derive(Debug)]
struct CompileOutput {
    kind: CompileOutputKind,
    function: CompiledFunction,
    index: u32,
}

#[derive(Debug)]
struct UnlinkedCompileOutputs {
    outputs: BTreeMap<CompileOutputKind, Vec<CompileOutput>>,
}

impl UnlinkedCompileOutputs {
    pub fn code_size_hint(&self) -> usize {
        self.outputs
            .iter()
            .map(|(_, vec)| vec.iter())
            .flatten()
            .fold(0, |acc, output| {
                acc + output.function.buffer.total_size() as usize
            })
    }

    pub fn num_funcs(&self) -> usize {
        self.outputs.iter().fold(0, |acc, (_, vec)| acc + vec.len())
    }

    pub fn link_and_append_code<'wasm, 'store>(
        mut self,
        module: Module<'wasm>,
        mut code_memory: CodeMemory<'store>,
    ) -> CompiledModule<'wasm, 'store> {
        let mut funcs = PrimaryMap::with_capacity(self.num_funcs());

        let compiled_functions = self.outputs.remove(&CompileOutputKind::Function).unwrap();

        let mut native_to_wasm_trampolines = self
            .outputs
            .remove(&CompileOutputKind::NativeToWasmTrampoline)
            .unwrap_or_default();

        let mut find_native_to_wasm_trampolines =
            |for_func: DefinedFuncIndex| -> Option<CompileOutput> {
                let index = native_to_wasm_trampolines.iter().position(|item| {
                    if item.index == for_func.as_u32() {
                        true
                    } else {
                        false
                    }
                });

                Some(native_to_wasm_trampolines.swap_remove(index?))
            };

        let mut relocs = Vec::new();
        for output in compiled_functions {
            relocs.extend(output.function.relocations());

            let def_func_index = DefinedFuncIndex::from_u32(output.index);
            let wasm_func_loc = code_memory.append_func(output.function);

            let native_to_wasm_trampoline = find_native_to_wasm_trampolines(def_func_index)
                .map(|output| code_memory.append_func(output.function));

            let new_index = funcs.push(CompiledFunctionInfo {
                wasm_func_loc,
                native_to_wasm_trampoline,
            });
            debug_assert_eq!(new_index, def_func_index);
        }

        log::debug!("relocs to process {relocs:?}");

        CompiledModule {
            code_memory,
            module,
            funcs,
        }
    }
}

/// Final compilation artifact
#[derive(Debug)]
pub struct CompiledModule<'wasm, 'store> {
    /// Finalized ELF object
    pub code_memory: CodeMemory<'store>,
    /// Type information about the compiled WebAssembly module.
    pub module: Module<'wasm>,
    /// Metadata about each compiled function.
    pub funcs: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
}

#[derive(Debug)]
pub struct CompiledFunctionInfo {
    /// The [`FunctionLoc`] indicating the location of this function in the text
    /// section of the compition artifact.
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

// struct CodeMemory<'store> {
//     inner: Vec<u8, &'store Store>,
// }
//
// impl<'store> CodeMemory<'store> {
//     pub fn with_capacity(capacity: usize, alloc: &'store GuestAllocator) -> Self {
//         Self {
//             inner: Vec::with_capacity_in(capacity, alloc),
//         }
//     }
//
//     pub fn len(&self) -> usize {
//         self.inner.len()
//     }
//
//     pub fn as_ptr(&self) -> *const u8 {
//         self.inner.as_ptr()
//     }
//
//     pub fn append_func(&mut self, func: CompiledFunction) -> FunctionLoc {
//         let loc = FunctionLoc {
//             start: self.len() as u32,
//             length: func.buffer.total_size(),
//         };
//
//         self.inner.extend_from_slice(func.buffer.data());
//
//         loc
//     }
//
//     pub fn publish() {}
// }
//
// impl<'store> Debug for CodeMemory<'store> {
//     fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
//         f.debug_tuple("CodeMemory")
//             .field(&format_args!(
//                 "{:?}, size: {}, capacity: {}",
//                 self.inner.as_ptr_range(),
//                 self.len(),
//                 self.inner.capacity()
//             ))
//             .finish()
//     }
// }
