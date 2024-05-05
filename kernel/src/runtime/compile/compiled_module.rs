use crate::runtime::compile::compiled_function::{CompiledFunction, RelocationTarget};
use crate::runtime::compile::{CompileKey, CompileOutput};
use crate::runtime::translate::Module;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use core::fmt::Formatter;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_wasm::DefinedFuncIndex;

/// Final compilation artifact
pub struct CompiledModule<'wasm> {
    /// The final, linked code artifact
    pub text: Vec<u8>,
    /// Type information about the compiled WebAssembly module.
    pub module: Module<'wasm>,
    /// Metadata about each compiled function.
    pub functions: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
}

impl<'wasm> fmt::Debug for CompiledModule<'wasm> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledModule")
            .field("text", &self.text.as_ptr_range())
            .field("module", &self.module)
            .field("functions", &self.functions)
            .finish()
    }
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

/// A helper to construct and link a coherent text segment from
/// a set of compiled functions
pub struct ModuleTextBuilder {
    text: Box<dyn cranelift_codegen::TextSectionBuilder>,
}

impl ModuleTextBuilder {
    pub fn new(text: Box<dyn cranelift_codegen::TextSectionBuilder>) -> Self {
        Self { text }
    }

    pub fn append_funcs<'a>(
        &mut self,
        funcs: impl Iterator<Item = &'a CompileOutput> + 'a,
        resolve_reloc_target: impl Fn(RelocationTarget) -> usize,
    ) -> PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> {
        let mut functions = PrimaryMap::new();

        for output in funcs {
            let wasm_func_loc = self.append_func(&output.function, &resolve_reloc_target);

            match output.key.kind() {
                CompileKey::WASM_FUNCTION_KIND => {
                    let def_func_index = DefinedFuncIndex::from_u32(output.key.index);
                    let new_def_func_idx = functions.push(CompiledFunctionInfo {
                        wasm_func_loc,
                        native_to_wasm_trampoline: None,
                    });
                    debug_assert_eq!(new_def_func_idx, def_func_index)
                }
                CompileKey::NATIVE_TO_WASM_TRAMPOLINE_KIND => {
                    let def_func_index = DefinedFuncIndex::from_u32(output.key.index);

                    functions
                        .get_mut(def_func_index)
                        .unwrap()
                        .native_to_wasm_trampoline = Some(wasm_func_loc);
                }
                _ => unreachable!(),
            }
        }

        functions
    }

    fn append_func(
        &mut self,
        compiled_func: &CompiledFunction,
        resolve_reloc_target: impl Fn(RelocationTarget) -> usize,
    ) -> FunctionLoc {
        let body = compiled_func.buffer.data();
        let alignment = compiled_func.alignment;
        let body_len = body.len() as u64;
        let off = self
            .text
            .append(true, &body, alignment, &mut Default::default());

        for r in compiled_func.relocations() {
            match r.target {
                // Relocations against user-defined functions means that this is
                // a relocation against a module-local function, typically a
                // call between functions. The `text` field is given priority to
                // resolve this relocation before we actually emit an object
                // file, but if it can't handle it then we pass through the
                // relocation.
                RelocationTarget::Wasm(_) | RelocationTarget::Builtin(_) => {
                    let target = resolve_reloc_target(r.target);
                    if self
                        .text
                        .resolve_reloc(off + u64::from(r.offset), r.kind, r.addend, target)
                    {
                        continue;
                    }

                    // At this time it's expected that all relocations are
                    // handled by `text.resolve_reloc`, and anything that isn't
                    // handled is a bug in `text.resolve_reloc` or something
                    // transitively there. If truly necessary, though, then this
                    // loop could also be updated to forward the relocation to
                    // the final object file as well.
                    panic!(
                        "unresolved relocation could not be processed against \
                         {:?}: {r:?}",
                        r.target,
                    );
                }
            }
        }

        FunctionLoc {
            start: off as u32,
            length: body_len as u32,
        }
    }
}
