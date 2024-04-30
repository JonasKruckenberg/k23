use crate::wasm::builtins::BuiltinFunctionIndex;
use crate::wasm::func_env::FuncEnvironment;
use crate::wasm::module::Module;
use crate::wasm::module_env::{FunctionBodyInput, ModuleTranslation};
use crate::wasm::{wasm_call_signature, FilePos, NS_WASM_BUILTIN, NS_WASM_FUNC};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::mem;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_codegen::ir::{
    ExternalName, GlobalValueData, MemFlags, SourceLoc, StackSlots, TrapCode, UserExternalName,
    UserExternalNameRef, UserFuncName,
};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::{
    binemit, ir, Context, Final, FinalizedMachReloc, FinalizedRelocTarget, MachBufferFinalized,
    MachSrcLoc, MachTrap, ValueLabelsRanges,
};
use cranelift_wasm::wasmparser::FuncValidatorAllocations;
use cranelift_wasm::{
    DefinedFuncIndex, FuncIndex, FuncTranslator, ModuleInternedTypeIndex, WasmSubType,
};

pub struct Compiler {
    target_isa: OwnedTargetIsa,
}

/// The compilation context for a single function
struct CompilationContext {
    func_translator: FuncTranslator,
    codegen_context: Context,
    // validator_allocations: FuncValidatorAllocations,
}

impl Default for CompilationContext {
    fn default() -> Self {
        Self {
            func_translator: FuncTranslator::new(),
            codegen_context: Context::new(),
            // validator_allocations: Default::default(),
        }
    }
}

impl Compiler {
    pub fn new(target_isa: OwnedTargetIsa) -> Self {
        Self { target_isa }
    }
    pub fn compile_function(
        &self,
        module: &Module,
        types: &PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
        def_func_index: DefinedFuncIndex,
        input: FunctionBodyInput,
    ) -> CompiledFunction {
        let target_isa = self.target_isa.as_ref();

        let mut ctx = CompilationContext::default();

        // Setup function signature
        let func_index = module.func_index(def_func_index);
        let sig_index = module.functions[func_index].signature;
        let wasm_func_ty = types[sig_index].unwrap_func();

        ctx.codegen_context.func.signature = wasm_call_signature(target_isa, wasm_func_ty);
        ctx.codegen_context.func.name = UserFuncName::User(UserExternalName {
            namespace: NS_WASM_FUNC,
            index: func_index.as_u32(),
        });

        // collect debug info
        ctx.codegen_context.func.collect_debug_info();

        let mut func_env = FuncEnvironment::new(target_isa, &module);

        // setup stack limit
        let vmctx = ctx
            .codegen_context
            .func
            .create_global_value(GlobalValueData::VMContext);
        let stack_limit = ctx
            .codegen_context
            .func
            .create_global_value(GlobalValueData::Load {
                base: vmctx,
                offset: i32::try_from(func_env.offsets.stack_limit())
                    .unwrap()
                    .into(),
                global_type: target_isa.pointer_type(),
                flags: MemFlags::trusted(),
            });
        ctx.codegen_context.func.stack_limit = Some(stack_limit);

        // translate the WASM function to cranelift IR
        let FunctionBodyInput {
            mut validator,
            body,
        } = input;

        ctx.func_translator
            .translate_body(
                &mut validator,
                body,
                &mut ctx.codegen_context.func,
                &mut func_env,
            )
            .unwrap();

        // compile cranelift IR to machine code
        let mut code_buf = Vec::new();
        let compiled_code = ctx
            .codegen_context
            .compile_and_emit(target_isa, &mut code_buf, &mut Default::default())
            .unwrap();

        let preferred_alignment = target_isa.function_alignment().preferred;
        let alignment = compiled_code.buffer.alignment.max(preferred_alignment);

        let mut compiled_func = CompiledFunction::new(
            compiled_code.buffer.clone(),
            ctx.codegen_context.func.params.user_named_funcs().clone(),
            alignment,
        );

        // set unwind info if requested
        // if target_isa.flags().unwind_info() {
        //     compiled_func.metadata.unwind_info =
        //         compiled_code.create_unwind_info(target_isa).unwrap();
        //
        //     // generate DWARF debug info too
        //     compiled_func.metadata.value_labels_ranges = compiled_code.value_labels_ranges.clone();
        // if !matches!(
        //     compiled_func.metadata().unwind_info,
        //     Some(UnwindInfo::SystemV(_))
        // ) {
        //     compiled_func.metadata.cfa_unwind_info = compiled_code
        //         .create_unwind_info_of_kind(target_isa, UnwindInfoKind::SystemV)
        //         .unwrap();
        // }
        // }

        compiled_func.metadata.sized_stack_slots =
            mem::take(&mut ctx.codegen_context.func.sized_stack_slots);

        compiled_func
    }
}

#[derive(Debug, Default)]
pub struct CompiledFunctionMetadata {
    // /// The unwind information.
    // pub unwind_info: Option<UnwindInfo>,
    // /// CFA-based unwind information for DWARF debugging support.
    // pub cfa_unwind_info: Option<CfaUnwindInfo>,
    /// Mapping of value labels and their locations.
    pub value_labels_ranges: ValueLabelsRanges,
    /// Allocated stack slots.
    pub sized_stack_slots: StackSlots,
    /// Start source location.
    pub start_srcloc: FilePos,
    /// End source location.
    pub end_srcloc: FilePos,
}

#[derive(Debug)]
pub struct CompiledFunction {
    /// The machine code buffer for this function.
    pub buffer: MachBufferFinalized<Final>,
    /// What names each name ref corresponds to.
    name_map: PrimaryMap<UserExternalNameRef, UserExternalName>,
    /// The alignment for the compiled function.
    pub alignment: u32,
    /// The metadata for the compiled function, including unwind information
    /// the function address map.
    metadata: CompiledFunctionMetadata,
}

impl CompiledFunction {
    /// Returns an iterator to the function's relocation information.
    pub fn relocations(&self) -> impl Iterator<Item = Relocation> + '_ {
        self.buffer
            .relocs()
            .iter()
            .map(|r| mach_reloc_to_reloc(r, &self.name_map))
    }

    // /// Get a reference to the unwind information from the
    // /// function's metadata.
    // pub fn unwind_info(&self) -> Option<&UnwindInfo> {
    //     self.metadata.unwind_info.as_ref()
    // }

    /// Get a reference to the compiled function metadata.
    pub fn metadata(&self) -> &CompiledFunctionMetadata {
        &self.metadata
    }

    fn new(
        buffer: MachBufferFinalized<Final>,
        name_map: PrimaryMap<UserExternalNameRef, UserExternalName>,
        alignment: u32,
    ) -> Self {
        Self {
            buffer,
            name_map,
            alignment,
            metadata: Default::default(),
        }
    }
}

#[derive(Debug)]
pub enum RelocationTarget {
    Wasm(FuncIndex),
    Builtin(BuiltinFunctionIndex),
}

#[derive(Debug)]
pub struct Relocation {
    kind: binemit::Reloc,
    target: RelocationTarget,
    addend: binemit::Addend,
    offset: binemit::CodeOffset,
}

fn mach_reloc_to_reloc(
    reloc: &FinalizedMachReloc,
    name_map: &PrimaryMap<UserExternalNameRef, UserExternalName>,
) -> Relocation {
    let &FinalizedMachReloc {
        offset,
        kind,
        ref target,
        addend,
    } = reloc;

    let target = match *target {
        FinalizedRelocTarget::ExternalName(ExternalName::User(user_func_ref)) => {
            let name = &name_map[user_func_ref];
            match name.namespace {
                // A reference to another jit'ed WASM function
                NS_WASM_FUNC => RelocationTarget::Wasm(FuncIndex::from_u32(name.index)),
                // A reference to a WASM builtin
                NS_WASM_BUILTIN => {
                    RelocationTarget::Builtin(BuiltinFunctionIndex::from_u32(name.index))
                }
                _ => panic!("unknown namespace {}", name.namespace),
            }
        }
        FinalizedRelocTarget::ExternalName(ExternalName::LibCall(libcall)) => {
            // cranelift libcalls are a lot like wasm builtins, they are emitted for instructions
            // that have no ISA equivalent and would be too complicated to emit as JIT code
            todo!("libcalls {libcall:?}")
        }
        _ => panic!("unsupported relocation target {target:?}"),
    };

    Relocation {
        kind,
        target,
        addend,
        offset,
    }
}
