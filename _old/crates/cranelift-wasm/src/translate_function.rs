use crate::state::State;
use crate::translate_inst::translate_inst;
use crate::{FuncTranslationEnvironment, TargetEnvironment};
use cranelift_codegen::ir;
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use wasmparser::FuncIdx;

pub fn translate_function(
    state: &mut State,
    func_builder_ctx: &mut FunctionBuilderContext,
    idx: FuncIdx,
    sig: ir::Signature,
    body: wasmparser::FunctionBody,
    env: &mut dyn FuncTranslationEnvironment,
) -> crate::Result<ir::Function> {
    let name = ir::UserFuncName::User(ir::UserExternalName {
        namespace: 0,
        index: idx.as_bits(),
    });

    let mut func = ir::Function::with_name_signature(name, sig);

    let mut builder = FunctionBuilder::new(&mut func, func_builder_ctx);

    translate_function_body(state, &mut builder, body, env)?;

    builder.finalize();
    state.reset();

    Ok(func)
}

fn translate_function_body(
    state: &mut State,
    builder: &mut FunctionBuilder,
    body: wasmparser::FunctionBody,
    env: &mut dyn FuncTranslationEnvironment,
) -> crate::Result<()> {
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.seal_block(entry);
    builder.switch_to_block(entry);

    let mut next_local = 0;

    log::trace!("declaring parameters...{:?}", builder.func.signature.params);
    declare_params(builder, &mut next_local, entry);

    log::trace!("declaring locals... {:?}", body.locals().unwrap());
    declare_and_define_locals(builder, &mut next_local, body.locals()?, env)?;

    for instr in body.instructions()? {
        log::trace!("translating instruction... {:?}", instr);

        translate_inst(state, builder, instr?, env)?;

        log::trace!(
            "after translate_instr reachable {:?} stack {:?} control stack {:?}",
            state.reachable,
            state.stack,
            state.control_stack
        );
    }

    Ok(())
}

fn declare_params(builder: &mut FunctionBuilder, next_local: &mut u32, block: ir::Block) {
    let params = builder.func.signature.params.clone();
    for param in params {
        let local = Variable::from_u32(*next_local);

        builder.declare_var(local, param.value_type);

        let val = builder.block_params(block)[*next_local as usize];
        builder.def_var(local, val);

        *next_local += 1;
    }
}

fn declare_and_define_locals(
    builder: &mut FunctionBuilder,
    next_local: &mut u32,
    locals: wasmparser::Locals,
    env: &dyn TargetEnvironment,
) -> crate::Result<()> {
    use wasmparser::ValueType;

    for locals in locals {
        let (count, ty) = locals?;

        for _ in 0..count {
            let local = Variable::from_u32(*next_local);

            let (ty, init) = match ty {
                ValueType::I32 => (types::I32, builder.ins().iconst(types::I32, 0)),
                ValueType::I64 => (types::I64, builder.ins().iconst(types::I64, 0)),
                ValueType::F32 => (types::F32, builder.ins().f32const(0.0)),
                ValueType::F64 => (types::F32, builder.ins().f64const(0.0)),
                ValueType::V128 => todo!(),
                ValueType::FuncRef | ValueType::ExternRef => (
                    env.target_config().pointer_type(),
                    builder.ins().iconst(env.target_config().pointer_type(), 0),
                ),
            };

            builder.declare_var(local, ty);
            builder.def_var(local, init);

            *next_local += 1;
        }
    }

    Ok(())
}
