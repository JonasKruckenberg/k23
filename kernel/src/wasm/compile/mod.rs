mod compile_key;
mod compiled_function;

use crate::wasm::builtins::BuiltinFunctionIndex;
use crate::wasm::compile::compiled_function::{RelocationTarget, TrapInfo};
use crate::wasm::indices::DefinedFuncIndex;
use crate::wasm::translate::{
    FunctionBodyData, ModuleTranslation, ModuleTypes, TranslatedModule, WasmFuncType,
};
use crate::wasm::trap::Trap;
use crate::wasm::Engine;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use compile_key::CompileKey;
pub use compiled_function::CompiledFunction;
use cranelift_codegen::control::ControlPlane;
use cranelift_entity::{EntitySet, PrimaryMap};

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_BUILTIN: u32 = 1;

pub trait Compiler {
    /// Returns the target triple this compiler is configured for
    fn triple(&self) -> &target_lexicon::Triple;

    fn text_section_builder(
        &self,
        capacity: usize,
    ) -> Box<dyn cranelift_codegen::TextSectionBuilder>;

    /// Compile the translated WASM function `index` within `translation`.
    fn compile_function(
        &self,
        translation: &ModuleTranslation<'_>,
        index: DefinedFuncIndex,
        data: FunctionBodyData<'_>,
        types: &ModuleTypes,
    ) -> crate::wasm::Result<CompiledFunction>;

    /// Compile a trampoline for calling the `index` WASM function through the
    /// array-calling convention used by host code to call into WASM.
    fn compile_array_to_wasm_trampoline(
        &self,
        translation: &ModuleTranslation<'_>,
        types: &ModuleTypes,
        index: DefinedFuncIndex,
    ) -> crate::wasm::Result<CompiledFunction>;

    // Compile a trampoline for calling the  a(host-defined) function through the array
    /// calling convention used to by WASM to call into host code.
    fn compile_wasm_to_array_trampoline(
        &self,
        wasm_func_ty: &WasmFuncType,
    ) -> crate::wasm::Result<CompiledFunction>;

    /// Compile a trampoline for calling the `index` builtin function from WASM.
    fn compile_wasm_to_builtin(
        &self,
        index: BuiltinFunctionIndex,
    ) -> crate::wasm::Result<CompiledFunction>;
}

/// A position within an original source file,
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FilePos(u32);

impl Default for FilePos {
    fn default() -> Self {
        Self(u32::MAX)
    }
}

impl FilePos {
    pub(crate) fn new(pos: u32) -> Self {
        Self(pos)
    }

    pub fn file_offset(self) -> Option<u32> {
        if self.0 == u32::MAX {
            None
        } else {
            Some(self.0)
        }
    }
}

#[derive(Debug)]
pub struct CompiledFunctionInfo {
    /// The [`FunctionLoc`] indicating the location of this function in the text
    /// section of the compilation artifact.
    pub wasm_func_loc: FunctionLoc,
    /// A trampoline for host callers (e.g. `Func::wrap`) calling into this function (if needed).
    pub host_to_wasm_trampoline: Option<FunctionLoc>,
    pub start_srcloc: FilePos,
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

pub type CompileInput<'a> =
    Box<dyn FnOnce(&dyn Compiler) -> crate::wasm::Result<CompileOutput> + Send + 'a>;

pub struct CompileInputs<'a>(Vec<CompileInput<'a>>);

impl<'a> CompileInputs<'a> {
    #[expect(tail_expr_drop_order, reason = "")]
    pub fn from_module(
        translation: &'a ModuleTranslation,
        types: &'a ModuleTypes,
        function_body_data: PrimaryMap<DefinedFuncIndex, FunctionBodyData<'a>>,
    ) -> Self {
        let mut inputs: Vec<CompileInput> = Vec::new();

        for (def_func_index, function_body_data) in function_body_data {
            // push the "main" function compilation job
            inputs.push(Box::new(move |compiler| {
                let symbol = format!("wasm[0]::function[{}]", def_func_index.as_u32());
                log::debug!("compiling {symbol}...");

                let function = compiler.compile_function(
                    translation,
                    def_func_index,
                    function_body_data,
                    types,
                )?;

                Ok(CompileOutput {
                    key: CompileKey::wasm_function(def_func_index),
                    function,
                    symbol,
                })
            }));

            // Compile a host->wasm trampoline for every function that are flags as "escaping"
            // and could therefore theoretically be called by native code.
            let func_index = translation.module.func_index(def_func_index);
            if translation.module.functions[func_index].is_escaping() {
                inputs.push(Box::new(move |compiler| {
                    let symbol =
                        format!("wasm[0]::array_to_wasm_trampoline[{}]", func_index.as_u32());
                    log::debug!("compiling {symbol}...");

                    let function = compiler.compile_array_to_wasm_trampoline(
                        translation,
                        types,
                        def_func_index,
                    )?;

                    Ok(CompileOutput {
                        key: CompileKey::array_to_wasm_trampoline(def_func_index),
                        function,
                        symbol,
                    })
                }));
            }
        }

        // TODO collect wasm->native trampolines

        Self(inputs)
    }

    #[expect(tail_expr_drop_order, reason = "")]
    pub fn compile(
        self,
        compiler: &dyn Compiler,
    ) -> crate::wasm::Result<UnlinkedCompileOutputs> {
        let mut outputs = self
            .0
            .into_iter()
            .map(|f| f(compiler))
            .collect::<Result<Vec<_>, _>>()?;

        compile_required_builtin_trampolines(compiler, &mut outputs)?;

        let mut indices: BTreeMap<u32, BTreeMap<CompileKey, usize>> = BTreeMap::new();
        for (index, output) in outputs.iter().enumerate() {
            indices
                .entry(output.key.kind())
                .or_default()
                .insert(output.key, index);
        }

        Ok(UnlinkedCompileOutputs { indices, outputs })
    }
}

fn compile_required_builtin_trampolines(
    compiler: &dyn Compiler,
    outputs: &mut Vec<CompileOutput>,
) -> crate::wasm::Result<()> {
    let mut builtins = EntitySet::new();
    let mut new_jobs: Vec<CompileInput<'_>> = Vec::new();

    let builtin_indices = outputs
        .iter()
        .flat_map(|output| output.function.relocations())
        .filter_map(|reloc| match reloc.target {
            RelocationTarget::Wasm(_) => None,
            RelocationTarget::Builtin(index) => Some(index),
        });

    let compile_builtin = |builtin: BuiltinFunctionIndex| -> CompileInput {
        Box::new(move |compiler: &dyn Compiler| {
            let symbol = format!("wasm_builtin_{}", builtin.name());
            log::debug!("compiling {symbol}...");
            Ok(CompileOutput {
                key: CompileKey::wasm_to_builtin_trampoline(builtin),
                symbol,
                function: compiler.compile_wasm_to_builtin(builtin)?,
            })
        })
    };

    for index in builtin_indices {
        if builtins.insert(index) {
            new_jobs.push(compile_builtin(index));
        }
    }

    outputs.extend(
        new_jobs
            .into_iter()
            .map(|f| f(compiler))
            .collect::<Result<Vec<_>, _>>()?,
    );

    Ok(())
}

#[derive(Debug)]
pub struct CompileOutput {
    pub key: CompileKey,
    pub function: CompiledFunction,
    pub symbol: String,
}

#[derive(Debug)]
pub struct UnlinkedCompileOutputs {
    indices: BTreeMap<u32, BTreeMap<CompileKey, usize>>,
    outputs: Vec<CompileOutput>,
}

impl UnlinkedCompileOutputs {
    #[expect(
        clippy::type_complexity,
        reason = "TODO clean up the return type and remove this"
    )]
    pub fn link_and_finish(
        mut self,
        engine: &Engine,
        module: &TranslatedModule,
    ) -> (
        Vec<u8>,
        PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
        (Vec<u32>, Vec<Trap>),
    ) {
        let mut text_builder = engine.compiler().text_section_builder(self.outputs.len());
        let mut ctrl_plane = ControlPlane::default();
        let mut locs = Vec::new(); // TODO get a capacity value for this
        let mut traps = TrapsBuilder::default();

        for output in &self.outputs {
            let body = output.function.buffer();
            let alignment = output.function.alignment();
            let body_len = body.len() as u64;
            let off = text_builder.append(true, body, alignment, &mut ctrl_plane);

            for r in output.function.relocations() {
                let target = match r.target {
                    RelocationTarget::Wasm(callee_index) => {
                        let def_func_index = module.defined_func_index(callee_index).unwrap();

                        self.indices[&CompileKey::WASM_FUNCTION_KIND]
                            [&CompileKey::wasm_function(def_func_index)]
                    }
                    RelocationTarget::Builtin(index) => {
                        self.indices[&CompileKey::WASM_TO_BUILTIN_TRAMPOLINE_KIND]
                            [&CompileKey::wasm_to_builtin_trampoline(index)]
                    }
                };

                // Ensure that we actually resolved the relocation
                debug_assert!(text_builder.resolve_reloc(
                    off + u64::from(r.offset),
                    r.kind,
                    r.addend,
                    target
                ));
            }

            let loc = FunctionLoc {
                start: u32::try_from(off).unwrap(),
                length: u32::try_from(body_len).unwrap(),
            };

            traps.push_traps(loc, output.function.traps());
            locs.push(loc);
        }

        let wasm_functions = self
            .indices
            .remove(&CompileKey::WASM_FUNCTION_KIND)
            .unwrap_or_default()
            .into_iter();

        let mut host_to_wasm_trampolines = self
            .indices
            .remove(&CompileKey::ARRAY_TO_WASM_TRAMPOLINE_KIND)
            .unwrap_or_default();

        let funcs = wasm_functions
            .map(|(key, index)| {
                let host_to_wasm_trampoline_key =
                    CompileKey::array_to_wasm_trampoline(DefinedFuncIndex::from_u32(key.index));
                let host_to_wasm_trampoline = host_to_wasm_trampolines
                    .remove(&host_to_wasm_trampoline_key)
                    .map(|index| locs[index]);

                CompiledFunctionInfo {
                    start_srcloc: self.outputs[index].function.metadata().start_srcloc,
                    wasm_func_loc: locs[index],
                    host_to_wasm_trampoline,
                }
            })
            .collect();

        (text_builder.finish(&mut ctrl_plane), funcs, traps.finish())
    }
}

#[derive(Default)]
struct TrapsBuilder {
    offsets: Vec<u32>,
    traps: Vec<Trap>,
    last_offset: u32,
}

impl TrapsBuilder {
    pub fn push_traps(
        &mut self,
        func: FunctionLoc,
        traps: impl ExactSizeIterator<Item = TrapInfo>,
    ) {
        self.offsets.reserve_exact(traps.len());
        self.traps.reserve_exact(traps.len());

        for trap in traps {
            let pos = func.start + trap.offset;
            debug_assert!(pos >= self.last_offset);
            // sanity check to make sure everything is sorted.
            // otherwise we won't be able to use lookup later.
            self.offsets.push(pos);
            self.traps.push(trap.trap);
            self.last_offset = pos;
        }

        self.last_offset = func.start + func.length;
    }

    pub fn finish(self) -> (Vec<u32>, Vec<Trap>) {
        (self.offsets, self.traps)
    }
}
