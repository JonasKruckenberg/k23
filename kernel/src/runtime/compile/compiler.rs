use crate::runtime::builtins::{BuiltinFunctionIndex, BuiltinFunctionSignatures};
use crate::runtime::compile::compiled_function::CompiledFunction;
use crate::runtime::translate::{FuncEnvironment, FunctionBodyInput, Module};
use crate::runtime::utils::{native_call_signature, wasm_call_signature};
use crate::runtime::vmcontext::{VMContextOffsets, VMCONTEXT_MAGIC};
use crate::runtime::{CompileError, DEBUG_ASSERT_TRAP_CODE, NS_WASM_FUNC};
use core::mem;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::I32;
use cranelift_codegen::ir::{
    Block, ExtFuncData, ExternalName, Function, GlobalValueData, Inst, InstBuilder, MemFlags,
    Signature, TrapCode, Type, UserExternalName, UserFuncName, Value,
};
use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_codegen::Context;
use cranelift_frontend::FunctionBuilder;
use cranelift_wasm::wasmparser::FuncValidatorAllocations;
use cranelift_wasm::{
    DefinedFuncIndex, FuncIndex, FuncTranslator, ModuleInternedTypeIndex, WasmSubType,
};

/// WASM to machine code compiler
pub struct Compiler {
    isa: OwnedTargetIsa,
}

impl Compiler {
    pub fn new(isa: OwnedTargetIsa) -> Self {
        Self { isa }
    }

    pub fn target_isa(&self) -> &dyn TargetIsa {
        self.isa.as_ref()
    }

    pub fn compile_function(
        &self,
        module: &Module,
        types: &PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
        def_func_index: DefinedFuncIndex,
        input: FunctionBodyInput,
    ) -> Result<CompiledFunction, CompileError> {
        let isa = self.target_isa();

        let mut ctx = CompilationContext::new(isa);

        // Setup function signature
        let func_index = module.func_index(def_func_index);
        let sig_index = module.functions[func_index].signature;
        let wasm_func_ty = types[sig_index].unwrap_func();

        ctx.codegen_context.func.signature = wasm_call_signature(isa, wasm_func_ty);
        ctx.codegen_context.func.name = UserFuncName::User(UserExternalName {
            namespace: NS_WASM_FUNC,
            index: func_index.as_u32(),
        });

        // collect debug info
        ctx.codegen_context.func.collect_debug_info();

        let mut func_env = FuncEnvironment::new(isa, &module);

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
                global_type: isa.pointer_type(),
                flags: MemFlags::trusted(),
            });
        ctx.codegen_context.func.stack_limit = Some(stack_limit);

        // translate the WASM function to cranelift IR
        let FunctionBodyInput { validator, body } = input;

        let mut validator = validator.into_validator(ctx.validator_allocations);
        ctx.func_translator.translate_body(
            &mut validator,
            body,
            &mut ctx.codegen_context.func,
            &mut func_env,
        )?;
        ctx.validator_allocations = validator.into_allocations();

        Ok(ctx.finish()?)
    }

    /// 1. save native stack pointer
    /// 2. call into Wasm
    /// 3. TODO return results
    pub fn compile_native_to_wasm_trampoline(
        &self,
        module: &Module,
        types: &PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
        def_func_index: DefinedFuncIndex,
    ) -> Result<CompiledFunction, CompileError> {
        let isa = self.target_isa();

        let func_index = module.func_index(def_func_index);
        let interned_index = module.functions[func_index].signature;
        let wasm_func_ty = types[interned_index].unwrap_func();
        let native_call_sig = native_call_signature(isa, wasm_func_ty);
        let wasm_call_sig = wasm_call_signature(isa, wasm_func_ty);

        let mut ctx = CompilationContext::new(isa);

        let func = Function::with_name_signature(Default::default(), native_call_sig);
        let (mut builder, block0) = ctx.new_function_builder(func);

        let args = builder.func.dfg.block_params(block0).to_vec();
        let vmctx = args[0];

        debug_assert_vmctx_kind(isa, &mut builder, vmctx, VMCONTEXT_MAGIC);

        // TODO lift this into some shared state?
        let offsets = VMContextOffsets::for_module(isa, &module);

        save_last_wasm_entry_sp(
            &mut builder,
            isa.pointer_type(),
            vmctx,
            offsets.last_wasm_entry_sp(),
        );

        // Call into WASM
        let call = declare_and_call(&mut builder, wasm_call_sig, func_index, &args);

        let results = builder.func.dfg.inst_results(call).to_vec();
        builder.ins().return_(&results);
        builder.finalize();

        Ok(ctx.finish()?)
    }

    pub fn compile_wasm_to_builtin_trampoline(
        &self,
        module: &Module,
        builtin_index: BuiltinFunctionIndex,
    ) -> Result<CompiledFunction, CompileError> {
        let isa = self.target_isa();

        let mut ctx = CompilationContext::new(isa);

        let builtin_call_sig = BuiltinFunctionSignatures::new(isa).signature(builtin_index);

        let func = Function::with_name_signature(Default::default(), builtin_call_sig.clone());
        let (mut builder, block0) = ctx.new_function_builder(func);

        let args = builder.func.dfg.block_params(block0).to_vec();
        let vmctx = args[0];
        let pointer_type = isa.pointer_type();

        debug_assert_vmctx_kind(isa, &mut builder, vmctx, VMCONTEXT_MAGIC);

        let offset = VMContextOffsets::for_module(isa, module);

        save_last_wasm_fp_and_pc(
            &mut builder,
            pointer_type,
            vmctx,
            offset.last_wasm_exit_fp(),
            offset.last_wasm_exit_pc(),
        );

        let mem_flags = MemFlags::trusted().with_readonly();
        let array_addr =
            builder
                .ins()
                .load(pointer_type, mem_flags, vmctx, offset.builtins() as i32);
        let func_addr = builder.ins().load(
            pointer_type,
            mem_flags,
            array_addr,
            (builtin_index.index() * pointer_type.bytes()) as i32,
        );

        let block_params = builder.block_params(block0).to_vec();
        let sig = builder.func.import_signature(builtin_call_sig);
        let call = builder.ins().call_indirect(sig, func_addr, &block_params);
        let results = builder.func.dfg.inst_results(call).to_vec();
        builder.ins().return_(&results);
        builder.finalize();

        Ok(ctx.finish()?)
    }
}

/// The compilation context for a single function
struct CompilationContext<'a> {
    target_isa: &'a dyn TargetIsa,
    func_translator: FuncTranslator,
    codegen_context: Context,
    validator_allocations: FuncValidatorAllocations,
}

impl<'a> CompilationContext<'a> {
    pub fn new(target_isa: &'a dyn TargetIsa) -> Self {
        Self {
            target_isa,
            func_translator: FuncTranslator::new(),
            codegen_context: Context::new(),
            validator_allocations: FuncValidatorAllocations::default(),
        }
    }

    pub fn new_function_builder(&mut self, func: Function) -> (FunctionBuilder, Block) {
        self.codegen_context.func = func;
        let mut builder = FunctionBuilder::new(
            &mut self.codegen_context.func,
            self.func_translator.context(),
        );

        let block0 = builder.create_block();
        builder.append_block_params_for_function_params(block0);
        builder.switch_to_block(block0);
        builder.seal_block(block0);
        (builder, block0)
    }

    pub fn finish(mut self) -> Result<CompiledFunction, CompileError> {
        let compiled_code = self
            .codegen_context
            .compile(self.target_isa, &mut Default::default())?;

        let preferred_alignment = self.target_isa.function_alignment().preferred;
        let alignment = compiled_code.buffer.alignment.max(preferred_alignment);

        let mut compiled_func = CompiledFunction::new(
            compiled_code.buffer.clone(),
            self.codegen_context.func.params.user_named_funcs().clone(),
            alignment,
        );

        compiled_func.metadata.sized_stack_slots =
            mem::take(&mut self.codegen_context.func.sized_stack_slots);

        Ok(compiled_func)
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
            I32,
            MemFlags::trusted().with_endianness(isa.endianness()),
            vmctx,
            0,
        );
        let is_expected_vmctx =
            builder
                .ins()
                .icmp_imm(IntCC::Equal, magic, i64::from(expected_vmctx_magic));
        builder
            .ins()
            .trapz(is_expected_vmctx, TrapCode::User(DEBUG_ASSERT_TRAP_CODE));
    }
}

fn save_last_wasm_entry_sp(
    builder: &mut FunctionBuilder,
    pointer_type: Type,
    vmctx: Value,
    last_wasm_entry_sp_offset: u32,
) {
    let sp = builder.ins().get_stack_pointer(pointer_type);
    builder.ins().store(
        MemFlags::trusted(),
        sp,
        vmctx,
        last_wasm_entry_sp_offset as i32,
    );
}

fn save_last_wasm_fp_and_pc(
    builder: &mut FunctionBuilder,
    pointer_type: Type,
    vmctx: Value,
    last_wasm_exit_fp_offset: u32,
    last_wasm_exit_pc_offset: u32,
) {
    let trampoline_fp = builder.ins().get_frame_pointer(pointer_type);
    let wasm_fp = builder
        .ins()
        .load(pointer_type, MemFlags::trusted(), trampoline_fp, 0);
    builder.ins().store(
        MemFlags::trusted(),
        wasm_fp,
        vmctx,
        last_wasm_exit_fp_offset as i32,
    );

    let wasm_pc = builder.ins().get_return_address(pointer_type);
    builder.ins().store(
        MemFlags::trusted(),
        wasm_pc,
        vmctx,
        last_wasm_exit_pc_offset as i32,
    );
}

fn declare_and_call(
    builder: &mut FunctionBuilder,
    signature: Signature,
    func_index: FuncIndex,
    args: &[Value],
) -> Inst {
    let name = ExternalName::User(
        builder
            .func
            .declare_imported_user_function(UserExternalName {
                namespace: NS_WASM_FUNC,
                index: func_index.as_u32(),
            }),
    );
    let signature = builder.func.import_signature(signature);
    let callee = builder.func.dfg.ext_funcs.push(ExtFuncData {
        name,
        signature,
        colocated: true,
    });
    builder.ins().call(callee, &args)
}
