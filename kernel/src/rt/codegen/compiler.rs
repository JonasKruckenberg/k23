use crate::rt::builtins::BuiltinFunctionIndex;
use crate::rt::codegen::func_env::FunctionEnvironment;
use crate::rt::codegen::module_env::FuncCompileInput;
use crate::rt::codegen::{CompiledFunction, TranslatedModule, ELFOSABI_K23};
use crate::rt::errors::CompileError;
use crate::rt::utils::wasm_call_signature;
use crate::rt::NS_WASM_FUNC;
use core::mem;
use cranelift_codegen::ir::{
    Block, Endianness, Function, GlobalValueData, MemFlags, UserExternalName, UserFuncName,
};
use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_codegen::Context;
use cranelift_entity::PrimaryMap;
use cranelift_frontend::FunctionBuilder;
use cranelift_wasm::wasmparser::FuncValidatorAllocations;
use cranelift_wasm::{DefinedFuncIndex, FuncTranslator, ModuleInternedTypeIndex, WasmSubType};
use object::write::Object;
use object::{BinaryFormat, FileFlags};
use target_lexicon::Architecture;

pub struct Compiler {
    isa: OwnedTargetIsa,
    func_translator: FuncTranslator,
    codegen_context: Context,
    validator_allocations: FuncValidatorAllocations,
}

impl Compiler {
    pub fn new(isa: OwnedTargetIsa) -> Compiler {
        Self {
            isa,
            func_translator: FuncTranslator::new(),
            codegen_context: Context::new(),
            validator_allocations: Default::default(),
        }
    }
    pub fn target_isa(&self) -> &dyn TargetIsa {
        self.isa.as_ref()
    }

    pub fn create_intermediate_code_object(&self) -> Object {
        let architecture = match self.isa.triple().architecture {
            Architecture::X86_32(_) => object::Architecture::I386,
            Architecture::X86_64 => object::Architecture::X86_64,
            Architecture::Arm(_) => object::Architecture::Arm,
            Architecture::Aarch64(_) => object::Architecture::Aarch64,
            Architecture::S390x => object::Architecture::S390x,
            Architecture::Riscv64(_) => object::Architecture::Riscv64,
            _ => panic!("unsupported"),
        };

        let endianness = match self.isa.endianness() {
            Endianness::Little => object::Endianness::Little,
            Endianness::Big => object::Endianness::Big,
        };

        let mut obj = Object::new(BinaryFormat::Elf, architecture, endianness);
        obj.flags = FileFlags::Elf {
            os_abi: ELFOSABI_K23,
            e_flags: 0,
            abi_version: 0,
        };

        obj
    }

    /// Compiles a WASM function
    pub fn compile_function(
        &self,
        module: &TranslatedModule,
        types: &PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
        def_func_index: DefinedFuncIndex,
        input: FuncCompileInput,
    ) -> Result<CompiledFunction, CompileError> {
        let isa = self.target_isa();

        let mut ctx = CompilationContext::new(isa);

        // Setup function signature
        let func_index = module.function_index(def_func_index);
        let sig_index = module.functions[func_index].signature;
        let wasm_func_ty = types[sig_index].unwrap_func();

        ctx.codegen_context.func.signature = wasm_call_signature(isa, wasm_func_ty);
        ctx.codegen_context.func.name = UserFuncName::User(UserExternalName {
            namespace: NS_WASM_FUNC,
            index: func_index.as_u32(),
        });

        // collect debug info
        ctx.codegen_context.func.collect_debug_info();

        let mut func_env = FunctionEnvironment::new(isa, &module, types);

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
                offset: i32::try_from(func_env.vmctx_stack_limit_offset())
                    .unwrap()
                    .into(),
                global_type: isa.pointer_type(),
                flags: MemFlags::trusted(),
            });
        ctx.codegen_context.func.stack_limit = Some(stack_limit);

        // translate the WASM function to cranelift IR
        let FuncCompileInput { validator, body } = input;

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

    /// Compiles a trampoline for calling a WASM function from the host
    pub fn compile_host_to_wasm_trampoline(&self) {}
    /// Compiles a trampoline for calling a host function from WASM
    pub fn compile_wasm_to_host_trampoline(&self) {}
    /// Compiles a trampoline for calling a builtin function from WASM
    pub fn compile_wasm_to_builtin_trampoline(
        &self,
        _module: &TranslatedModule,
        _builtin_index: BuiltinFunctionIndex,
    ) -> Result<CompiledFunction, CompileError> {
        todo!()
    }
}

/// Ad-hoc structure for compiling a single input
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
