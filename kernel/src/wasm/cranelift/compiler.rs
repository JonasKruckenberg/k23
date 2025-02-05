use crate::arch;
use crate::wasm::builtins::BuiltinFunctionIndex;
use crate::wasm::compile::{CompiledFunction, Compiler, FilePos, NS_WASM_FUNC};
use crate::wasm::cranelift::builtins::BuiltinFunctionSignatures;
use crate::wasm::cranelift::env::TranslationEnvironment;
use crate::wasm::cranelift::func_translator::FuncTranslator;
use crate::wasm::indices::DefinedFuncIndex;
use crate::wasm::runtime::{StaticVMOffsets, VMCONTEXT_MAGIC};
use crate::wasm::translate::{
    FunctionBodyData, ModuleTranslation, ModuleTypes, WasmFuncType, WasmValType,
};
use crate::wasm::trap::{TRAP_EXIT, TRAP_INTERNAL_ASSERT};
use crate::wasm::utils::{array_call_signature, value_type, wasm_call_signature};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt::Formatter;
use core::{cmp, fmt, mem};
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::{Endianness, InstBuilder, Type, Value};
use cranelift_codegen::ir::{GlobalValueData, MemFlags, Signature, UserExternalName, UserFuncName};
use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_codegen::{ir, TextSectionBuilder};
use cranelift_frontend::FunctionBuilder;
use sync::Mutex;
use target_lexicon::Triple;
use wasmparser::{FuncValidatorAllocations, FunctionBody};

pub struct CraneliftCompiler {
    isa: OwnedTargetIsa,
    contexts: Mutex<Vec<CompilationContext>>,
    offsets: StaticVMOffsets,
}

impl fmt::Debug for CraneliftCompiler {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("CraneliftCompiler").finish_non_exhaustive()
    }
}

impl CraneliftCompiler {
    #[expect(tail_expr_drop_order, reason = "")]
    pub(crate) fn new(isa: OwnedTargetIsa) -> CraneliftCompiler {
        Self {
            offsets: StaticVMOffsets::new(isa.pointer_bytes()),
            isa,
            contexts: Mutex::new(Vec::new()), // TODO capacity should be equal to the number of cpus
        }
    }

    fn target_isa(&self) -> &dyn TargetIsa {
        self.isa.as_ref()
    }

    #[expect(tail_expr_drop_order, reason = "")]
    fn function_compiler(&self) -> FunctionCompiler<'_> {
        let saved_context = self.contexts.lock().pop();
        FunctionCompiler {
            compiler: self,
            ctx: saved_context
                .map(|mut ctx| {
                    ctx.codegen_context.clear();
                    ctx
                })
                .unwrap_or_default(),
        }
    }
}

impl Compiler for CraneliftCompiler {
    fn triple(&self) -> &Triple {
        self.isa.triple()
    }

    fn text_section_builder(&self, capacity: usize) -> Box<dyn TextSectionBuilder> {
        self.isa.text_section_builder(capacity)
    }

    fn compile_function(
        &self,
        translation: &ModuleTranslation<'_>,
        index: DefinedFuncIndex,
        data: FunctionBodyData<'_>,
        types: &ModuleTypes,
    ) -> crate::wasm::Result<CompiledFunction> {
        let isa = self.target_isa();
        let mut compiler = self.function_compiler();
        let context = &mut compiler.ctx.codegen_context;

        // Setup function signature
        let index = translation.module.func_index(index);
        let sig_index = translation.module.functions[index].signature;
        let func_ty = types
            .get_wasm_type(translation.module.types[sig_index])
            .unwrap()
            .unwrap_func();

        context.func.signature = wasm_call_signature(isa, func_ty);
        context.func.name = UserFuncName::User(UserExternalName {
            namespace: NS_WASM_FUNC,
            index: index.as_u32(),
        });

        // set up stack limit
        let vmctx = context.func.create_global_value(GlobalValueData::VMContext);
        let stack_limit = context.func.create_global_value(GlobalValueData::Load {
            base: vmctx,
            offset: i32::from(self.offsets.vmctx_stack_limit()).into(),
            global_type: isa.pointer_type(),
            flags: MemFlags::trusted(),
        });
        context.func.stack_limit = Some(stack_limit);

        // collect debug info
        context.func.collect_debug_info();

        let mut env = TranslationEnvironment::new(isa, &translation.module, types);
        let mut validator = data
            .validator
            .into_validator(mem::take(&mut compiler.ctx.validator_allocations));
        compiler.ctx.func_translator.translate_body(
            &mut validator,
            &data.body,
            &mut context.func,
            &mut env,
        )?;

        compiler.finish(Some(&data.body))
    }

    fn compile_array_to_wasm_trampoline(
        &self,
        translation: &ModuleTranslation<'_>,
        types: &ModuleTypes,
        index: DefinedFuncIndex,
    ) -> crate::wasm::Result<CompiledFunction> {
        // This function has a special calling convention where all arguments and return values
        // are passed through an array in memory (so we can have dynamic function signatures in rust)
        let pointer_type = self.isa.pointer_type();
        let index = translation.module.func_index(index);
        let sig_index = translation.module.functions[index].signature;
        let func_ty = types
            .get_wasm_type(translation.module.types[sig_index])
            .unwrap()
            .unwrap_func();

        let wasm_call_sig = wasm_call_signature(self.target_isa(), func_ty);
        let array_call_sig = array_call_signature(self.target_isa());

        let mut compiler = self.function_compiler();
        let func = ir::Function::with_name_signature(UserFuncName::default(), array_call_sig);
        let (mut builder, block0) = compiler.builder(func);

        let (vmctx, caller_vmctx, values_vec_ptr, values_vec_len) = {
            let params = builder.func.dfg.block_params(block0);
            (params[0], params[1], params[2], params[3])
        };

        // First load the actual arguments out of the array.
        let mut args = load_values_from_array(
            &func_ty.params,
            &mut builder,
            values_vec_ptr,
            values_vec_len,
            pointer_type,
        );
        args.insert(0, caller_vmctx);
        args.insert(0, vmctx);

        // Assert that we were really given a core Wasm vmctx, since that's
        // what we are assuming with our offsets below.
        debug_assert_vmctx_kind(self.target_isa(), &mut builder, vmctx, VMCONTEXT_MAGIC);
        // Then store our current stack pointer into the appropriate slot.
        let fp = builder.ins().get_frame_pointer(pointer_type);
        builder.ins().store(
            MemFlags::trusted(),
            fp,
            vmctx,
            i32::from(self.offsets.vmctx_last_wasm_entry_fp()),
        );

        // Then call the Wasm function with those arguments.
        let call = declare_and_call(&mut builder, wasm_call_sig, index.as_u32(), &args);
        let results = builder.func.dfg.inst_results(call).to_vec();

        store_values_to_array(
            &mut builder,
            &func_ty.results,
            &results,
            values_vec_ptr,
            values_vec_len,
        );

        builder.ins().trap(TRAP_EXIT);
        // builder.ins().return_(&[]);
        builder.finalize();

        compiler.finish(None)
    }

    fn compile_wasm_to_array_trampoline(
        &self,
        wasm_func_ty: &WasmFuncType,
    ) -> crate::wasm::Result<CompiledFunction> {
        let pointer_type = self.isa.pointer_type();
        let wasm_call_sig = wasm_call_signature(self.target_isa(), wasm_func_ty);
        let _array_call_sig = array_call_signature(self.target_isa());

        let mut compiler = self.function_compiler();
        let func = ir::Function::with_name_signature(UserFuncName::default(), wasm_call_sig);
        let (mut builder, block0) = compiler.builder(func);

        let args = builder.func.dfg.block_params(block0).to_vec();
        let _callee_vmctx = args[0];
        let _caller_vmctx = args[1];

        // Spill all wasm arguments to the stack in `ValRaw` slots.
        let (_args_base, args_len) = allocate_stack_array_and_spill_args(
            wasm_func_ty,
            &mut builder,
            &args[2..],
            pointer_type,
        );
        let _args_len = builder.ins().iconst(pointer_type, i64::from(args_len));

        // TODO figure out address of host func (from host func vmctx)
        // TODO call indirect with [callee_vmctx, caller_vmctx, args_base, args_len]

        todo!()
    }

    fn compile_wasm_to_builtin(
        &self,
        index: BuiltinFunctionIndex,
    ) -> crate::wasm::Result<CompiledFunction> {
        let isa = &*self.isa;
        let pointer_type = isa.pointer_type();
        let sig = BuiltinFunctionSignatures::new(isa).signature(index);

        let mut compiler = self.function_compiler();
        let func = ir::Function::with_name_signature(UserFuncName::default(), sig.clone());
        let (mut builder, block0) = compiler.builder(func);
        let vmctx = builder.block_params(block0)[0];

        // Debug-assert that this is the right kind of vmctx, and then
        // additionally perform the "routine of the exit trampoline" of saving
        // fp/pc/etc.
        debug_assert_vmctx_kind(isa, &mut builder, vmctx, VMCONTEXT_MAGIC);
        save_last_wasm_exit_fp_and_pc(&mut builder, pointer_type, &self.offsets, vmctx);

        // Now it's time to delegate to the actual builtin. Builtins are stored
        // in an array in all `VMContext`s. First load the base pointer of the
        // array and then load the entry of the array that corresponds to this
        // builtin.
        let mem_flags = MemFlags::trusted().with_readonly();
        let array_addr = builder.ins().load(
            pointer_type,
            mem_flags,
            vmctx,
            i32::from(self.offsets.vmctx_builtin_functions()),
        );
        let func_offset = i32::try_from(index.as_u32().wrapping_mul(pointer_type.bytes())).unwrap();
        let func_addr = builder
            .ins()
            .load(pointer_type, mem_flags, array_addr, func_offset);

        // Forward all our own arguments to the libcall itself, and then return
        // all the same results as the libcall.
        let block_params = builder.block_params(block0).to_vec();
        let sig = builder.func.import_signature(sig);
        let call = builder.ins().call_indirect(sig, func_addr, &block_params);
        let results = builder.func.dfg.inst_results(call).to_vec();
        builder.ins().return_(&results);
        builder.finalize();

        compiler.finish(None)
    }
}

struct CompilationContext {
    func_translator: FuncTranslator,
    codegen_context: cranelift_codegen::Context,
    validator_allocations: FuncValidatorAllocations,
}

impl Default for CompilationContext {
    fn default() -> Self {
        Self {
            func_translator: FuncTranslator::new(),
            codegen_context: cranelift_codegen::Context::new(),
            validator_allocations: FuncValidatorAllocations::default(),
        }
    }
}

struct FunctionCompiler<'a> {
    compiler: &'a CraneliftCompiler,
    ctx: CompilationContext,
}

impl FunctionCompiler<'_> {
    fn builder(&mut self, func: ir::Function) -> (FunctionBuilder<'_>, ir::Block) {
        self.ctx.codegen_context.func = func;
        let mut builder = FunctionBuilder::new(
            &mut self.ctx.codegen_context.func,
            self.ctx.func_translator.context_mut(),
        );

        let block0 = builder.create_block();
        builder.append_block_params_for_function_params(block0);
        builder.switch_to_block(block0);
        builder.seal_block(block0);
        (builder, block0)
    }

    fn finish(mut self, body: Option<&FunctionBody<'_>>) -> crate::wasm::Result<CompiledFunction> {
        let context = &mut self.ctx.codegen_context;

        context.set_disasm(true);
        let compiled_code =
            context.compile(self.compiler.target_isa(), &mut ControlPlane::default())?;

        let preferred_alignment = self.compiler.isa.function_alignment().preferred;
        let alignment = compiled_code.buffer.alignment.max(preferred_alignment);
        let mut compiled_function = CompiledFunction::new(
            compiled_code.buffer.clone(),
            context.func.params.user_named_funcs().clone(),
            alignment,
        );

        compiled_function.metadata_mut().sized_stack_slots =
            mem::take(&mut context.func.sized_stack_slots);

        if let Some(body) = body {
            let reader = body.get_binary_reader();
            let offset = reader.original_position();
            let len = reader.bytes_remaining();

            compiled_function.metadata_mut().start_srcloc =
                FilePos::new(u32::try_from(offset).unwrap());
            compiled_function.metadata_mut().end_srcloc =
                FilePos::new(u32::try_from(offset + len).unwrap());

            // TODO
            // let srclocs = compiled_function
            //     .buffer
            //     .get_srclocs_sorted()
            //     .into_iter()
            //     .map(|&MachSrcLoc { start, end, loc }| (loc, start, end - start));

            // compiled_function.metadata_mut().address_map = collect_address_map(
            //     u32::try_from(compiled_function.buffer.data().len()).unwrap(),
            //     srclocs,
            // )
            //     .into_boxed_slice();
        }

        self.ctx.codegen_context.clear();
        self.compiler.contexts.lock().push(self.ctx);

        Ok(compiled_function)
    }
}

// Helper function for declaring a cranelift function
/// and immediately inserting a call instruction.
fn declare_and_call(
    builder: &mut FunctionBuilder,
    signature: Signature,
    func_index: u32,
    args: &[Value],
) -> ir::Inst {
    let name = ir::ExternalName::User(builder.func.declare_imported_user_function(
        UserExternalName {
            namespace: NS_WASM_FUNC,
            index: func_index,
        },
    ));
    let signature = builder.func.import_signature(signature);
    let callee = builder.func.dfg.ext_funcs.push(ir::ExtFuncData {
        name,
        signature,
        colocated: true,
    });
    builder.ins().call(callee, args)
}

fn save_last_wasm_exit_fp_and_pc(
    builder: &mut FunctionBuilder,
    pointer_type: Type,
    offsets: &StaticVMOffsets,
    vmctx: Value,
) {
    // Save the exit Wasm FP to the limits. We dereference the current FP to get
    // the previous FP because the current FP is the trampoline's FP, and we
    // want the Wasm function's FP, which is the caller of this trampoline.
    let trampoline_fp = builder.ins().get_frame_pointer(pointer_type);
    let wasm_fp = builder.ins().load(
        pointer_type,
        MemFlags::trusted(),
        trampoline_fp,
        i32::try_from(arch::NEXT_OLDER_FP_FROM_FP_OFFSET).unwrap(),
    );
    builder.ins().store(
        MemFlags::trusted(),
        wasm_fp,
        vmctx,
        Offset32::new(i32::from(offsets.vmctx_last_wasm_exit_fp())),
    );
    // Finally save the Wasm return address to the limits.
    let wasm_pc = builder.ins().get_return_address(pointer_type);
    builder.ins().store(
        MemFlags::trusted(),
        wasm_pc,
        vmctx,
        Offset32::new(i32::from(offsets.vmctx_last_wasm_exit_pc())),
    );
}

fn allocate_stack_array_and_spill_args(
    ty: &WasmFuncType,
    builder: &mut FunctionBuilder,
    args: &[Value],
    pointer_type: Type,
) -> (Value, u32) {
    // Compute the size of the values vector.
    let value_size = size_of::<u128>();
    let values_vec_len = cmp::max(ty.params.len(), ty.results.len());
    let values_vec_byte_size = u32::try_from(value_size.wrapping_mul(values_vec_len)).unwrap();
    let values_vec_len = u32::try_from(values_vec_len).unwrap();

    let slot = builder.func.create_sized_stack_slot(ir::StackSlotData::new(
        ir::StackSlotKind::ExplicitSlot,
        values_vec_byte_size,
        4,
    ));
    let values_vec_ptr = builder.ins().stack_addr(pointer_type, slot, 0i32);

    {
        let values_vec_len = builder
            .ins()
            .iconst(ir::types::I32, i64::from(values_vec_len));
        store_values_to_array(builder, &ty.params, args, values_vec_ptr, values_vec_len);
    }

    (values_vec_ptr, values_vec_len)
}

/// Used for loading the values of an array-call host function's value
/// array.
///
/// This can be used to load arguments out of the array if the trampoline we
/// are building exposes the array calling convention, or it can be used to
/// load results out of the array if the trampoline we are building calls a
/// function that uses the array calling convention.
fn load_values_from_array(
    types: &[WasmValType],
    builder: &mut FunctionBuilder,
    values_vec_ptr: Value,
    values_vec_capacity: Value,
    pointer_type: Type,
) -> Vec<Value> {
    let value_size = size_of::<u128>();

    debug_assert_enough_capacity_for_length(builder, types.len(), values_vec_capacity);

    // Note that this is little-endian like `store_values_to_array` above,
    // see notes there for more information.
    let flags = MemFlags::new()
        .with_notrap()
        .with_endianness(Endianness::Little);

    let mut results = Vec::new();
    for (i, ty) in types.iter().enumerate() {
        let ir_ty = value_type(ty, pointer_type);
        let val = builder.ins().load(
            ir_ty,
            flags,
            values_vec_ptr,
            i32::try_from(i.wrapping_mul(value_size)).unwrap(),
        );
        results.push(val);
    }
    results
}

/// Store values to an array in the array calling convention.
///
/// Used either to store arguments to the array when calling a function
/// using the array calling convention, or used to store results to the
/// array when implementing a function that exposes the array calling
/// convention.
fn store_values_to_array(
    builder: &mut FunctionBuilder,
    types: &[WasmValType],
    values: &[Value],
    values_vec_ptr: Value,
    values_vec_capacity: Value,
) {
    debug_assert_eq!(types.len(), values.len());
    debug_assert_enough_capacity_for_length(builder, types.len(), values_vec_capacity);

    let flags = MemFlags::new()
        .with_notrap()
        .with_endianness(Endianness::Little);

    let value_size = size_of::<u128>();
    for (i, val) in values.iter().copied().enumerate() {
        builder.ins().store(
            flags,
            val,
            values_vec_ptr,
            i32::try_from(i.wrapping_mul(value_size)).unwrap(),
        );
    }
}

fn debug_assert_enough_capacity_for_length(
    builder: &mut FunctionBuilder,
    length: usize,
    capacity: Value,
) {
    if cfg!(debug_assertions) {
        let enough_capacity = builder.ins().icmp_imm(
            ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
            capacity,
            ir::immediates::Imm64::new(length.try_into().unwrap()),
        );
        builder.ins().trapz(enough_capacity, TRAP_INTERNAL_ASSERT);
    }
}

fn debug_assert_vmctx_kind(
    isa: &dyn TargetIsa,
    builder: &mut FunctionBuilder,
    vmctx: Value,
    expected_vmctx_magic: u32,
) {
    if cfg!(debug_assertions) {
        let magic = builder.ins().load(
            ir::types::I32,
            MemFlags::trusted().with_endianness(isa.endianness()),
            vmctx,
            0i32,
        );
        let is_expected_vmctx = builder.ins().icmp_imm(
            ir::condcodes::IntCC::Equal,
            magic,
            i64::from(expected_vmctx_magic),
        );
        builder.ins().trapz(is_expected_vmctx, TRAP_INTERNAL_ASSERT);
    }
}
