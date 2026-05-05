// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use cranelift_codegen::ir;
use cranelift_codegen::ir::{InstBuilder, ValueLabel};
use cranelift_entity::EntityRef;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use wasmparser::{BinaryReader, FuncValidator, FunctionBody, WasmModuleResources};

use crate::wasm::cranelift::code_translator::{bitcast_wasm_returns, translate_operator};
use crate::wasm::cranelift::env::TranslationEnvironment;
use crate::wasm::cranelift::state::FuncTranslationState;
use crate::wasm::cranelift::utils::get_vmctx_value_label;

pub struct FuncTranslator {
    func_ctx: FunctionBuilderContext,
    state: FuncTranslationState,
}

impl FuncTranslator {
    /// Create a new translator.
    pub fn new() -> Self {
        Self {
            func_ctx: FunctionBuilderContext::new(),
            state: FuncTranslationState::new(),
        }
    }

    pub(crate) fn context_mut(&mut self) -> &mut FunctionBuilderContext {
        &mut self.func_ctx
    }

    pub fn translate_body(
        &mut self,
        validator: &mut FuncValidator<impl WasmModuleResources>,
        body: &FunctionBody<'_>,
        func: &mut ir::Function,
        env: &mut TranslationEnvironment,
    ) -> crate::Result<()> {
        let mut reader = body.get_binary_reader();
        tracing::trace!(
            "parsing {} bytes, {}{}",
            reader.bytes_remaining(),
            func.name,
            func.signature
        );
        debug_assert_eq!(func.dfg.num_blocks(), 0, "Function must be empty");
        debug_assert_eq!(func.dfg.num_insts(), 0, "Function must be empty");

        let mut builder = FunctionBuilder::new(func, &mut self.func_ctx);
        builder.set_srcloc(cur_srcloc(&reader));
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block); // Declare all predecessors known.

        // Make sure the entry block is inserted in the layout before we make any callbacks to
        // `environ`. The callback functions may need to insert things in the entry block.
        builder.ensure_inserted_block();

        let num_params = declare_wasm_parameters(&mut builder, entry_block, env);

        // Set up the translation state with a single pushed control block representing the whole
        // function and its return values.
        let exit_block = builder.create_block();
        builder.append_block_params_for_function_returns(exit_block);
        self.state.initialize(&builder.func.signature, exit_block);

        translate_local_decls(&mut reader, &mut builder, num_params, validator, env)?;
        translate_function_body(validator, reader, &mut builder, &mut self.state, env)?;

        builder.finalize();
        tracing::trace!("translated Wasm to CLIF:\n{}", func.display());
        Ok(())
    }
}

fn declare_wasm_parameters(
    builder: &mut FunctionBuilder,
    entry_block: ir::Block,
    env: &mut TranslationEnvironment,
) -> usize {
    let sig_len = builder.func.signature.params.len();
    let mut next_local = 0;
    for i in 0..sig_len {
        let param_type = builder.func.signature.params[i];
        // There may be additional special-purpose parameters in addition to the normal WebAssembly
        // signature parameters. For example, a `vmctx` pointer.
        if env.is_wasm_parameter(i) {
            // This is a normal WebAssembly signature parameter, so create a local for it.
            let local = Variable::new(next_local);
            builder.declare_var(local, param_type.value_type);
            // This is checked by validation to not overflow
            next_local += 1;

            let param_value = builder.block_params(entry_block)[i];
            builder.def_var(local, param_value);
        }
        if param_type.purpose == ir::ArgumentPurpose::VMContext {
            let param_value = builder.block_params(entry_block)[i];
            builder.set_val_label(param_value, get_vmctx_value_label());
        }
    }

    next_local
}

fn translate_local_decls(
    reader: &mut BinaryReader,
    builder: &mut FunctionBuilder,
    num_params: usize,
    validator: &mut FuncValidator<impl WasmModuleResources>,
    env: &mut TranslationEnvironment,
) -> crate::Result<()> {
    let mut next_local = num_params;
    let local_count = reader.read_var_u32()?;

    for _ in 0..local_count {
        builder.set_srcloc(cur_srcloc(reader));
        let pos = reader.original_position();
        let count = reader.read_var_u32()?;
        let ty = reader.read()?;
        validator.define_locals(pos, count, ty)?;
        declare_locals(builder, count, ty, &mut next_local, env);
    }

    Ok(())
}

fn declare_locals(
    builder: &mut FunctionBuilder,
    count: u32,
    wasm_type: wasmparser::ValType,
    next_local: &mut usize,
    env: &mut TranslationEnvironment,
) {
    use wasmparser::ValType::*;

    // All locals are initialized to 0.
    let (ty, init, needs_stack_map) = match wasm_type {
        I32 => (
            ir::types::I32,
            Some(builder.ins().iconst(ir::types::I32, 0)),
            false,
        ),
        I64 => (
            ir::types::I64,
            Some(builder.ins().iconst(ir::types::I64, 0)),
            false,
        ),
        F32 => (
            ir::types::F32,
            Some(builder.ins().f32const(ir::immediates::Ieee32::with_bits(0))),
            false,
        ),
        F64 => (
            ir::types::F64,
            Some(builder.ins().f64const(ir::immediates::Ieee64::with_bits(0))),
            false,
        ),
        V128 => {
            let constant_handle = builder.func.dfg.constants.insert([0; 16].to_vec().into());
            (
                ir::types::I8X16,
                Some(builder.ins().vconst(ir::types::I8X16, constant_handle)),
                false,
            )
        }
        Ref(rt) => {
            let hty = env.convert_heap_type(rt.heap_type());
            let (ty, needs_stack_map) = env.reference_type(&hty);
            let init = if rt.is_nullable() {
                Some(env.translate_ref_null(builder.cursor(), &hty))
            } else {
                None
            };
            (ty, init, needs_stack_map)
        }
    };

    for _ in 0..count {
        let local = Variable::new(*next_local);
        builder.declare_var(local, ty);
        if needs_stack_map {
            builder.declare_var_needs_stack_map(local);
        }
        if let Some(init) = init {
            builder.def_var(local, init);
            builder.set_val_label(init, ValueLabel::new(*next_local));
        }
        // This is checked by validation to not overflow
        *next_local += 1;
    }
}

/// Parse the function body in `reader`.
///
/// This assumes that the local variable declarations have already been parsed and function
/// arguments and locals are declared in the builder.
fn translate_function_body(
    validator: &mut FuncValidator<impl WasmModuleResources>,
    mut reader: BinaryReader,
    builder: &mut FunctionBuilder,
    state: &mut FuncTranslationState,
    env: &mut TranslationEnvironment,
) -> crate::Result<()> {
    // The control stack is initialized with a single block representing the whole function.
    debug_assert_eq!(state.control_stack.len(), 1, "State not initialized");

    while !reader.eof() {
        let pos = reader.original_position();
        builder.set_srcloc(cur_srcloc(&reader));
        let op = reader.read_operator()?;
        translate_operator(validator, &op, builder, state, env)?;
        validator.op(pos, &op)?;
    }
    // env.after_translate_function(builder, state)?;
    let pos = reader.original_position();
    validator.finish(pos)?;

    // The final `End` operator left us in the exit block where we need to manually add a return
    // instruction.
    //
    // If the exit block is unreachable, it may not have the correct arguments, so we would
    // generate a return instruction that doesn't match the signature.
    if state.reachable && !builder.is_unreachable() {
        bitcast_wasm_returns(&mut state.stack, builder, env);
        builder.ins().return_(&state.stack);
    }

    // Discard any remaining values on the stack. Either we just returned them,
    // or the end of the function is unreachable.
    state.stack.clear();

    Ok(())
}

/// Get the current source location from a reader.
fn cur_srcloc(reader: &BinaryReader) -> ir::SourceLoc {
    // We record source locations as byte code offsets relative to the beginning of the file.
    ir::SourceLoc::new(u32::try_from(reader.original_position()).unwrap())
}
