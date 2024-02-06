use crate::error::ensure;
use crate::heap::Heap;
use crate::state::{ControlFrame, ElseState, GlobalVariable, State};
use crate::traits::FuncTranslationEnvironment;
use crate::Error;
use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::{
    types, Fact, InstBuilder, MemFlags, Opcode, RelSourceLoc, TrapCode, Type, Value,
};
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::EntityRef;
use cranelift_frontend::{FunctionBuilder, Variable};

pub fn translate_inst(
    state: &mut State,
    builder: &mut FunctionBuilder,
    inst: wasmparser::Instruction,
    env: &mut dyn FuncTranslationEnvironment,
) -> crate::Result<()> {
    use wasmparser::Instruction;

    if !state.reachable {
        return translate_unreachable_inst(state, builder, inst, env);
    }

    match inst {
        Instruction::Unreachable => {
            builder.ins().trap(TrapCode::UnreachableCodeReached);
        }
        Instruction::Nop => {} // Do nothing
        Instruction::Drop => {
            let _ = state.pop1()?;
        }
        Instruction::Select | Instruction::TypedSelect { .. } => {
            let (x, y, cond) = state.pop3()?;
            state.push1(builder.ins().select(cond, x, y));
        }

        Instruction::LocalGet { local } => {
            let var = Variable::from_u32(local.as_bits());
            let val = builder.use_var(var);
            state.push1(val);
        }
        Instruction::LocalSet { local } => {
            let var = Variable::from_u32(local.as_bits());
            let val = state.pop1()?;
            builder.def_var(var, val);
        }
        Instruction::LocalTee { local } => {
            let var = Variable::from_u32(local.as_bits());
            let val = state.peek1()?;
            builder.def_var(var, val);
        }

        Instruction::GlobalGet { global } => {
            let val = match state.get_or_make_global(builder.cursor(), global, env)? {
                GlobalVariable::Const(val) => val,
                GlobalVariable::Memory { gv, offset, ty } => {
                    let base = builder.ins().global_value(ty, gv);

                    let mut flags = MemFlags::trusted();
                    flags.set_table();

                    builder.ins().load(ty, flags, base, offset)
                }
                GlobalVariable::Host => env.translate_global_get(builder.cursor(), global)?,
            };

            state.push1(val);
        }
        Instruction::GlobalSet { global } => {
            let val = state.pop1()?;

            match state.get_or_make_global(builder.cursor(), global, env)? {
                GlobalVariable::Const(_) => panic!("assignment to constant global"),
                GlobalVariable::Memory { gv, offset, ty } => {
                    let base = builder.ins().global_value(ty, gv);

                    let mut flags = MemFlags::trusted();
                    flags.set_table();

                    builder.ins().store(flags, val, base, offset);
                }
                GlobalVariable::Host => {
                    env.translate_global_set(builder.cursor(), global, val)?;
                }
            }
        }

        Instruction::Block { ty } => {
            let (params, returns) = block_type_params_returns(&ty, env);
            let next_block = block_with_params(builder, returns.clone(), env);
            state.push_block(next_block, params.count(), returns.count());
        }
        Instruction::Loop { ty } => {
            let (params, returns) = block_type_params_returns(&ty, env);
            let loop_body = block_with_params(builder, params.clone(), env);
            let next_block = block_with_params(builder, returns.clone(), env);

            let num_params = params.count();
            builder.ins().jump(loop_body, state.peekn(num_params)?);

            state.push_loop(loop_body, next_block, num_params, returns.count());

            state.popn(num_params)?;
            state
                .stack
                .extend_from_slice(builder.block_params(loop_body));

            builder.switch_to_block(loop_body);
        }
        Instruction::If { ty } => {
            let cond = state.pop1()?;

            let (params, returns) = block_type_params_returns(&ty, env);
            let num_params = params.clone().count();

            let consequent = builder.create_block(); // the if's consequent
            let next_block = block_with_params(builder, returns.clone(), env); // this is the block *after* the if

            // TODO why?
            let else_state = if params.clone().eq(returns.clone()) {
                let branch_inst =
                    builder
                        .ins()
                        .brif(cond, consequent, &[], next_block, state.peekn(num_params)?);

                ElseState::Absent {
                    branch_inst,
                    placeholder: next_block,
                }
            } else {
                let else_block = block_with_params(builder, params, env);

                builder
                    .ins()
                    .brif(cond, consequent, &[], else_block, state.peekn(num_params)?);

                ElseState::Present { else_block }
            };

            builder.seal_block(consequent);
            builder.switch_to_block(consequent);

            state.push_if(next_block, else_state, ty, num_params, returns.count())
        }
        Instruction::Else => {
            let i = state.control_stack.len() - 1;
            let ControlFrame::If {
                ref mut is_consequent_end_reachable,
                is_consequent_start_reachable,
                ref else_state,
                block_type,
                next_block,
                num_returns,
                ..
            } = state.control_stack[i]
            else {
                panic!("expected if control frame on top of stack")
            };

            // We finished the consequent, so record its final
            // reachability state.
            debug_assert!(is_consequent_end_reachable.is_none());
            *is_consequent_end_reachable = Some(state.reachable);

            if is_consequent_start_reachable {
                state.reachable = true;

                let else_block = match *else_state {
                    ElseState::Absent {
                        branch_inst,
                        placeholder,
                    } => {
                        let (params, _) = block_type_params_returns(&block_type, env);
                        let else_block = block_with_params(builder, params.clone(), env);
                        let num_params = params.count();

                        let jump_args = state.peekn(num_params)?;
                        builder.ins().jump(next_block, jump_args);
                        state.popn(num_params)?;

                        builder.change_jump_destination(branch_inst, placeholder, else_block);
                        builder.seal_block(else_block);
                        else_block
                    }
                    ElseState::Present { else_block } => {
                        let jump_args = state.peekn(num_returns)?;
                        builder.ins().jump(next_block, jump_args);
                        state.popn(num_returns)?;
                        else_block
                    }
                };

                builder.switch_to_block(else_block);
            }
        }
        Instruction::End => {
            if let Some(frame) = state.control_stack.pop() {
                let next_block = frame.next_block();
                let num_returns = frame.num_returns();

                log::debug!("stack before jump {:?}", state.stack);
                builder.ins().jump(next_block, state.peekn(num_returns)?);

                if let ControlFrame::Loop { body, .. } = frame {
                    builder.seal_block(body);
                }

                builder.switch_to_block(next_block);
                builder.seal_block(next_block);

                log::debug!("truncating stack to {}", frame.original_stack_size());
                state.truncate_value_stack_to_original_size(&frame);

                log::debug!("next_block_params {:?}", builder.block_params(next_block));
                state
                    .stack
                    .extend_from_slice(builder.block_params(next_block));
            } else {
                log::debug!("ending last block");
                // we're ending the last block of the whole function
                if state.reachable && !builder.is_unreachable() {
                    builder.ins().return_(&state.stack);
                    state.stack.clear();
                }
            }
        }

        Instruction::Br { label } => {
            ensure!(
                state.control_stack.len() > label.index(),
                Error::EmptyControlStack
            );
            let i = state.control_stack.len() - 1 - label.index();
            let frame = &mut state.control_stack[i];

            log::debug!("frame is {frame:?}");

            frame.set_branched_to_exit();

            let num_returns = if frame.is_loop() {
                frame.num_params()
            } else {
                frame.num_returns()
            };

            let br_destination = frame.br_destination();

            let jump_args = state.peekn(num_returns)?;
            builder.ins().jump(br_destination, jump_args);
            state.popn(num_returns)?;

            state.reachable = false;
        }
        Instruction::BrIf { label } => {
            let cond = state.pop1()?;

            ensure!(
                state.control_stack.len() > label.index(),
                Error::EmptyControlStack
            );
            let i = state.control_stack.len() - 1 - label.index();
            let frame = &mut state.control_stack[i];

            frame.set_branched_to_exit();

            let num_returns = if frame.is_loop() {
                frame.num_params()
            } else {
                frame.num_returns()
            };

            let next_block = builder.create_block();
            let br_destination = frame.br_destination();

            let args = state.peekn(num_returns)?;
            builder
                .ins()
                .brif(cond, br_destination, args, next_block, &[]);
            state.popn(num_returns)?;

            builder.seal_block(next_block);
            builder.switch_to_block(next_block);
        }
        Instruction::BrTable { .. } => todo!(),
        Instruction::Call { function } => {
            let (func_ref, num_args) =
                state.get_or_make_direct_func(builder.func, function, env)?;

            let args = state.peekn(num_args)?;
            let inst = env.translate_call(builder, function, func_ref, args)?;
            state.popn(num_args)?;

            state.pushn(builder.func.dfg.inst_results(inst));
        }
        Instruction::ReturnCall { function } => {
            let (func_ref, num_args) =
                state.get_or_make_direct_func(builder.func, function, env)?;

            let args = state.peekn(num_args)?;
            env.translate_return_call(builder, function, func_ref, args)?;
            state.popn(num_args)?;
        }
        Instruction::CallIndirect { ty, table } => {
            let (sig_ref, num_args) = state.get_or_make_indirect_sig(builder.func, ty, env)?;
            let itable = state.get_or_make_table(builder.func, table, env)?;

            let callee = state.pop1()?;

            let args = state.peekn(num_args)?;
            let inst =
                env.translate_call_indirect(builder, table, itable, ty, sig_ref, callee, args)?;
            state.popn(num_args)?;

            state.pushn(builder.func.dfg.inst_results(inst));
        }
        Instruction::ReturnCallIndirect { ty, table } => {
            let (sig_ref, num_args) = state.get_or_make_indirect_sig(builder.func, ty, env)?;
            let itable = state.get_or_make_table(builder.func, table, env)?;

            let callee = state.pop1()?;

            let args = state.peekn(num_args)?;
            env.translate_return_call_indirect(builder, table, itable, ty, sig_ref, callee, args)?;
            state.popn(num_args)?;
        }
        Instruction::Return => {
            if let Some(frame) = state.control_stack.get(0) {
                let num_returns = frame.num_returns();
                let return_args = state.peekn(num_returns)?;
                builder.ins().return_(return_args);
                state.popn(num_returns)?;
            } else {
                builder.ins().return_(&state.stack);
                state.stack.clear();
            }

            state.reachable = false;
        }

        // Instruction::CallRef { .. } => {}
        // Instruction::ReturnCallRef { .. } => {}
        // Instruction::RefAsNonNull => {}
        // Instruction::BrOnNull { .. } => {}
        // Instruction::BrOnNonNull { .. } => {}
        // Instruction::RefNull { .. } => {}
        // Instruction::RefIsNull => {}
        // Instruction::RefFunc { .. } => {}
        //
        // Instruction::Try { .. } => {}
        // Instruction::Catch { .. } => {}
        // Instruction::Throw { .. } => {}
        // Instruction::Rethrow { .. } => {}
        // Instruction::Delegate { .. } => {}
        // Instruction::CatchAll => {}
        Instruction::I32Const { value } => {
            state.push1(builder.ins().iconst(types::I32, value as i64))
        }
        Instruction::I64Const { value } => {
            state.push1(builder.ins().iconst(types::I64, value as i64))
        }
        Instruction::F32Const { value } => state.push1(builder.ins().f32const(value.as_f32())),
        Instruction::F64Const { value } => state.push1(builder.ins().f64const(value.as_f64())),

        Instruction::I32Eqz | Instruction::I64Eqz => {
            let val = state.pop1()?;
            let val = builder.ins().icmp_imm(IntCC::Equal, val, 0);
            state.push1(builder.ins().uextend(types::I32, val))
        }
        Instruction::I32Eq | Instruction::I64Eq => translate_icmp(state, builder, IntCC::Equal)?,
        Instruction::I32Ne | Instruction::I64Ne => translate_icmp(state, builder, IntCC::NotEqual)?,
        Instruction::I32LtS | Instruction::I64LtS => {
            translate_icmp(state, builder, IntCC::SignedLessThan)?
        }
        Instruction::I32LtU | Instruction::I64LtU => {
            translate_icmp(state, builder, IntCC::UnsignedLessThan)?
        }
        Instruction::I32GtS | Instruction::I64GtS => {
            translate_icmp(state, builder, IntCC::SignedGreaterThan)?
        }
        Instruction::I32GtU | Instruction::I64GtU => {
            translate_icmp(state, builder, IntCC::UnsignedGreaterThan)?
        }
        Instruction::I32LeS | Instruction::I64LeS => {
            translate_icmp(state, builder, IntCC::SignedLessThanOrEqual)?
        }
        Instruction::I32LeU | Instruction::I64LeU => {
            translate_icmp(state, builder, IntCC::UnsignedLessThanOrEqual)?
        }
        Instruction::I32GeS | Instruction::I64GeS => {
            translate_icmp(state, builder, IntCC::SignedGreaterThanOrEqual)?
        }
        Instruction::I32GeU | Instruction::I64GeU => {
            translate_icmp(state, builder, IntCC::UnsignedGreaterThanOrEqual)?
        }

        Instruction::F32Eq | Instruction::F64Eq => translate_fcmp(state, builder, FloatCC::Equal)?,
        Instruction::F32Ne | Instruction::F64Ne => {
            translate_fcmp(state, builder, FloatCC::NotEqual)?
        }
        Instruction::F32Lt | Instruction::F64Lt => {
            translate_fcmp(state, builder, FloatCC::LessThan)?
        }
        Instruction::F32Gt | Instruction::F64Gt => {
            translate_fcmp(state, builder, FloatCC::GreaterThan)?
        }
        Instruction::F32Le | Instruction::F64Le => {
            translate_fcmp(state, builder, FloatCC::LessThanOrEqual)?
        }
        Instruction::F32Ge | Instruction::F64Ge => {
            translate_fcmp(state, builder, FloatCC::GreaterThanOrEqual)?
        }

        Instruction::I32Clz | Instruction::I64Clz => {
            translate_unary_arith(state, builder, Opcode::Clz)?
        }
        Instruction::I32Ctz | Instruction::I64Ctz => {
            translate_unary_arith(state, builder, Opcode::Ctz)?
        }
        Instruction::I32Popcnt | Instruction::I64Popcnt => {
            translate_unary_arith(state, builder, Opcode::Popcnt)?
        }

        Instruction::I32Add | Instruction::I64Add => {
            translate_binary_arith(state, builder, Opcode::Iadd)?
        }
        Instruction::I32Sub | Instruction::I64Sub => {
            translate_binary_arith(state, builder, Opcode::Isub)?
        }
        Instruction::I32Mul | Instruction::I64Mul => {
            translate_binary_arith(state, builder, Opcode::Imul)?
        }
        Instruction::I32DivS | Instruction::I64DivS => {
            translate_binary_arith(state, builder, Opcode::Sdiv)?
        }
        Instruction::I32DivU | Instruction::I64DivU => {
            translate_binary_arith(state, builder, Opcode::Udiv)?
        }
        Instruction::I32RemS | Instruction::I64RemS => {
            translate_binary_arith(state, builder, Opcode::Srem)?
        }
        Instruction::I32RemU | Instruction::I64RemU => {
            translate_binary_arith(state, builder, Opcode::Urem)?
        }
        Instruction::I32And | Instruction::I64And => {
            translate_binary_arith(state, builder, Opcode::Band)?
        }
        Instruction::I32Or | Instruction::I64Or => {
            translate_binary_arith(state, builder, Opcode::Bor)?
        }
        Instruction::I32Xor | Instruction::I64Xor => {
            translate_binary_arith(state, builder, Opcode::Bxor)?
        }
        Instruction::I32Shl | Instruction::I64Shl => {
            translate_binary_arith(state, builder, Opcode::Ishl)?
        }
        Instruction::I32ShrS | Instruction::I64ShrS => {
            translate_binary_arith(state, builder, Opcode::Sshr)?
        }
        Instruction::I32ShrU | Instruction::I64ShrU => {
            translate_binary_arith(state, builder, Opcode::Ushr)?
        }
        Instruction::I32Rotl | Instruction::I64Rotl => {
            translate_binary_arith(state, builder, Opcode::Rotl)?
        }
        Instruction::I32Rotr | Instruction::I64Rotr => {
            translate_binary_arith(state, builder, Opcode::Rotr)?
        }

        Instruction::F32Abs | Instruction::F64Abs => {
            translate_unary_arith(state, builder, Opcode::Fabs)?
        }
        Instruction::F32Neg | Instruction::F64Neg => {
            translate_unary_arith(state, builder, Opcode::Fneg)?
        }
        Instruction::F32Ceil | Instruction::F64Ceil => {
            translate_unary_arith(state, builder, Opcode::Ceil)?
        }
        Instruction::F32Floor | Instruction::F64Floor => {
            translate_unary_arith(state, builder, Opcode::Floor)?
        }
        Instruction::F32Trunc | Instruction::F64Trunc => {
            translate_unary_arith(state, builder, Opcode::Trunc)?
        }
        Instruction::F32Nearest | Instruction::F64Nearest => {
            translate_unary_arith(state, builder, Opcode::Nearest)?
        }
        Instruction::F32Sqrt | Instruction::F64Sqrt => {
            translate_unary_arith(state, builder, Opcode::Sqrt)?
        }
        Instruction::F32Add | Instruction::F64Add => {
            translate_binary_arith(state, builder, Opcode::Fadd)?
        }
        Instruction::F32Sub | Instruction::F64Sub => {
            translate_binary_arith(state, builder, Opcode::Fsub)?
        }
        Instruction::F32Mul | Instruction::F64Mul => {
            translate_binary_arith(state, builder, Opcode::Fmul)?
        }
        Instruction::F32Div | Instruction::F64Div => {
            translate_binary_arith(state, builder, Opcode::Fdiv)?
        }
        Instruction::F32Min | Instruction::F64Min => {
            translate_binary_arith(state, builder, Opcode::Fmin)?
        }
        Instruction::F32Max | Instruction::F64Max => {
            translate_binary_arith(state, builder, Opcode::Fmax)?
        }
        Instruction::F32Copysign | Instruction::F64Copysign => {
            translate_binary_arith(state, builder, Opcode::Fcopysign)?
        }
        Instruction::I32WrapI64 => {
            let val = state.pop1()?;
            state.push1(builder.ins().ireduce(types::I32, val));
        }
        Instruction::I32TruncF32S | Instruction::I32TruncF64S => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_to_sint(types::I32, val));
        }
        Instruction::I32TruncF32U | Instruction::I32TruncF64U => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_to_uint(types::I32, val));
        }
        Instruction::I64ExtendI32S => {
            let val = state.pop1()?;
            state.push1(builder.ins().sextend(types::I64, val))
        }
        Instruction::I64ExtendI32U => {
            let val = state.pop1()?;
            state.push1(builder.ins().uextend(types::I64, val))
        }
        Instruction::I64TruncF32S | Instruction::I64TruncF64S => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_to_sint(types::I64, val));
        }
        Instruction::I64TruncF32U | Instruction::I64TruncF64U => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_to_uint(types::I64, val));
        }
        Instruction::F32ConvertI32S | Instruction::F32ConvertI64S => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_from_uint(types::F32, val))
        }
        Instruction::F32ConvertI32U | Instruction::F32ConvertI64U => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_from_uint(types::F32, val))
        }
        Instruction::F32DemoteF64 => {
            let val = state.pop1()?;
            state.push1(builder.ins().fdemote(types::F32, val));
        }
        Instruction::F64ConvertI32S | Instruction::F64ConvertI64S => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_from_sint(types::F64, val))
        }
        Instruction::F64ConvertI32U | Instruction::F64ConvertI64U => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_from_uint(types::F64, val))
        }
        Instruction::F64PromoteF32 => {
            let val = state.pop1()?;
            state.push1(builder.ins().fpromote(types::F64, val));
        }
        Instruction::I32ReinterpretF32 => {
            let val = state.pop1()?;
            state.push1(builder.ins().bitcast(types::I32, MemFlags::new(), val));
        }
        Instruction::I64ReinterpretF64 => {
            let val = state.pop1()?;
            state.push1(builder.ins().bitcast(types::I64, MemFlags::new(), val));
        }
        Instruction::F32ReinterpretI32 => {
            let val = state.pop1()?;
            state.push1(builder.ins().bitcast(types::F32, MemFlags::new(), val));
        }
        Instruction::F64ReinterpretI64 => {
            let val = state.pop1()?;
            state.push1(builder.ins().bitcast(types::F64, MemFlags::new(), val));
        }
        Instruction::I32Extend8S | Instruction::I32Extend16S => {
            let val = state.pop1()?;
            state.push1(builder.ins().sextend(types::I32, val));
        }
        Instruction::I64Extend8S | Instruction::I64Extend16S | Instruction::I64Extend32S => {
            let val = state.pop1()?;
            state.push1(builder.ins().sextend(types::I64, val));
        }

        Instruction::I32TruncSatF32S | Instruction::I32TruncSatF64S => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_to_sint(types::I32, val));
        }
        Instruction::I32TruncSatF32U | Instruction::I32TruncSatF64U => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_to_uint(types::I32, val));
        }
        Instruction::I64TruncSatF32S | Instruction::I64TruncSatF64S => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_to_sint(types::I64, val));
        }
        Instruction::I64TruncSatF32U | Instruction::I64TruncSatF64U => {
            let val = state.pop1()?;
            state.push1(builder.ins().fcvt_to_uint(types::I64, val));
        }

        Instruction::I32Load { memarg } => {
            translate_load(state, builder, Opcode::Load, types::I32, memarg, env)?
        }
        Instruction::I64Load { memarg } => {
            translate_load(state, builder, Opcode::Load, types::I64, memarg, env)?
        }
        Instruction::F32Load { memarg } => {
            translate_load(state, builder, Opcode::Load, types::F32, memarg, env)?
        }
        Instruction::F64Load { memarg } => {
            translate_load(state, builder, Opcode::Load, types::F64, memarg, env)?
        }
        Instruction::I32Load8S { memarg } => {
            translate_load(state, builder, Opcode::Sload8, types::I32, memarg, env)?
        }
        Instruction::I32Load8U { memarg } => {
            translate_load(state, builder, Opcode::Uload8, types::I32, memarg, env)?
        }
        Instruction::I32Load16S { memarg } => {
            translate_load(state, builder, Opcode::Sload16, types::I32, memarg, env)?
        }
        Instruction::I32Load16U { memarg } => {
            translate_load(state, builder, Opcode::Uload16, types::I32, memarg, env)?
        }
        Instruction::I64Load8S { memarg } => {
            translate_load(state, builder, Opcode::Sload8, types::I64, memarg, env)?
        }
        Instruction::I64Load8U { memarg } => {
            translate_load(state, builder, Opcode::Uload8, types::I64, memarg, env)?
        }
        Instruction::I64Load16S { memarg } => {
            translate_load(state, builder, Opcode::Sload16, types::I64, memarg, env)?
        }
        Instruction::I64Load16U { memarg } => {
            translate_load(state, builder, Opcode::Uload16, types::I64, memarg, env)?
        }
        Instruction::I64Load32S { memarg } => {
            translate_load(state, builder, Opcode::Sload32, types::I64, memarg, env)?
        }
        Instruction::I64Load32U { memarg } => {
            translate_load(state, builder, Opcode::Uload32, types::I64, memarg, env)?
        }
        Instruction::I32Store { memarg }
        | Instruction::I64Store { memarg }
        | Instruction::F32Store { memarg }
        | Instruction::F64Store { memarg } => {
            translate_store(state, builder, Opcode::Store, memarg, env)?
        }
        Instruction::I32Store8 { memarg } | Instruction::I64Store8 { memarg } => {
            translate_store(state, builder, Opcode::Istore8, memarg, env)?
        }
        Instruction::I32Store16 { memarg } | Instruction::I64Store16 { memarg } => {
            translate_store(state, builder, Opcode::Istore16, memarg, env)?
        }
        Instruction::I64Store32 { memarg } => {
            translate_store(state, builder, Opcode::Istore32, memarg, env)?
        }
        Instruction::MemoryInit { data, mem } => {
            let (dst, src, len) = state.pop3()?;
            let heap = state.get_or_make_heap(builder.func, mem, env)?;
            env.translate_memory_init(builder.cursor(), mem, heap, data, dst, src, len)?;
        }
        Instruction::MemoryGrow { mem } => {
            let delta = state.pop1()?;
            let heap = state.get_or_make_heap(builder.func, mem, env)?;
            let val = env.translate_memory_grow(builder.cursor(), mem, heap, delta)?;
            state.push1(val);
        }
        Instruction::MemorySize { mem } => {
            let heap = state.get_or_make_heap(builder.func, mem, env)?;
            let val = env.translate_memory_size(builder.cursor(), mem, heap)?;
            state.push1(val);
        }
        Instruction::MemoryCopy { dst_mem, src_mem } => {
            let (dst, src, len) = state.pop3()?;
            let dst_heap = state.get_or_make_heap(builder.func, dst_mem, env)?;
            let src_heap = state.get_or_make_heap(builder.func, src_mem, env)?;
            env.translate_memory_copy(
                builder.cursor(),
                dst_mem,
                dst_heap,
                src_mem,
                src_heap,
                dst,
                src,
                len,
            )?;
        }
        Instruction::MemoryFill { mem } => {
            let (dst, val, len) = state.pop3()?;
            let heap = state.get_or_make_heap(builder.func, mem, env)?;
            env.translate_memory_fill(builder.cursor(), mem, heap, dst, val, len)?;
        }
        Instruction::DataDrop { data } => {
            env.translate_data_drop(builder.cursor(), data)?;
        }
        Instruction::MemoryDiscard { mem } => {
            let heap = state.get_or_make_heap(builder.func, mem, env)?;
            env.translate_memory_discard(builder.cursor(), mem, heap)?;
        }
        Instruction::TableInit { element, table } => {
            let (dst, src, len) = state.pop3()?;
            let itable = state.get_or_make_table(builder.func, table, env)?;
            env.translate_table_init(builder.cursor(), table, itable, element, dst, src, len)?;
        }
        Instruction::TableGrow { table } => {
            let (init, delta) = state.pop2()?;
            let itable = state.get_or_make_table(builder.func, table, env)?;
            let val = env.translate_table_grow(builder.cursor(), table, itable, delta, init)?;
            state.push1(val);
        }
        Instruction::TableGet { table } => {
            let index = state.pop1()?;
            let itable = state.get_or_make_table(builder.func, table, env)?;
            let val = env.translate_table_get(builder, table, itable, index)?;
            state.push1(val);
        }
        Instruction::TableSet { table } => {
            let (index, val) = state.pop2()?;
            let itable = state.get_or_make_table(builder.func, table, env)?;
            env.translate_table_set(builder, table, itable, index, val)?;
        }
        Instruction::TableSize { table } => {
            let itable = state.get_or_make_table(builder.func, table, env)?;
            let val = env.translate_table_size(builder.cursor(), table, itable)?;
            state.push1(val);
        }
        Instruction::TableCopy {
            dst_table,
            src_table,
        } => {
            let (dst, src, len) = state.pop3()?;
            let dst_itable = state.get_or_make_table(builder.func, dst_table, env)?;
            let src_itable = state.get_or_make_table(builder.func, src_table, env)?;
            env.translate_table_copy(
                builder.cursor(),
                dst_table,
                dst_itable,
                src_table,
                src_itable,
                dst,
                src,
                len,
            )?;
        }
        Instruction::TableFill { table } => {
            let (dst, val, len) = state.pop3()?;
            let itable = state.get_or_make_table(builder.func, table, env)?;
            env.translate_table_fill(builder.cursor(), table, itable, dst, val, len)?;
        }
        Instruction::ElemDrop { element } => {
            env.translate_elem_drop(builder.cursor(), element)?;
        }

        _ => todo!("implement instruction {inst:?}"),
    }

    Ok(())
}

fn translate_unreachable_inst(
    state: &mut State,
    builder: &mut FunctionBuilder,
    inst: wasmparser::Instruction,
    env: &mut dyn FuncTranslationEnvironment,
) -> crate::Result<()> {
    use wasmparser::Instruction;

    debug_assert!(state.reachable);

    match inst {
        Instruction::If { ty } => {
            state.push_if(
                ir::Block::reserved_value(),
                ElseState::Absent {
                    branch_inst: ir::Inst::reserved_value(),
                    placeholder: ir::Block::reserved_value(),
                },
                ty,
                0,
                0,
            );
        }
        Instruction::Loop { .. } | Instruction::Block { .. } => {
            state.push_block(ir::Block::reserved_value(), 0, 0);
        }
        Instruction::Else => {
            let i = state.control_stack.len() - 1;
            let ControlFrame::If {
                ref mut is_consequent_end_reachable,
                is_consequent_start_reachable,
                ref else_state,
                block_type,
                original_stack_size,
                ..
            } = state.control_stack[i]
            else {
                panic!("expected if control frame on top of stack")
            };

            // We finished the consequent, so record its final
            // reachability state.
            debug_assert!(is_consequent_end_reachable.is_none());
            *is_consequent_end_reachable = Some(state.reachable);

            if is_consequent_start_reachable {
                state.reachable = true;

                let else_block = match *else_state {
                    ElseState::Absent {
                        branch_inst,
                        placeholder,
                    } => {
                        let (params, _results) = block_type_params_returns(&block_type, env);
                        let else_block = block_with_params(builder, params, env);

                        state.stack.truncate(original_stack_size);

                        // We change the target of the branch instruction.
                        builder.change_jump_destination(branch_inst, placeholder, else_block);
                        builder.seal_block(else_block);
                        else_block
                    }
                    ElseState::Present { else_block } => {
                        state.stack.truncate(original_stack_size);
                        else_block
                    }
                };

                builder.switch_to_block(else_block);
            }
        }
        Instruction::End => {
            // do usual end instruction stuff but also determine whether this end restores reachability

            if let Some(frame) = state.control_stack.pop() {
                // whether the next block is reachable
                log::debug!("truncating stack to {}", frame.original_stack_size());
                state.truncate_value_stack_to_original_size(&frame);

                let next_reachable = match frame {
                    ControlFrame::Loop { body, .. } => {
                        builder.seal_block(body);
                        false
                    }
                    ControlFrame::If {
                        is_consequent_start_reachable,
                        is_consequent_end_reachable,
                        ..
                    } => {
                        if let Some(is_end_reachable) = is_consequent_end_reachable {
                            is_consequent_start_reachable && is_end_reachable
                        } else {
                            is_consequent_start_reachable
                        }
                    }
                    _ => false,
                };

                if frame.exit_is_branched_to() || next_reachable {
                    builder.switch_to_block(frame.next_block());
                    builder.seal_block(frame.next_block());

                    log::debug!(
                        "next_block_params {:?}",
                        builder.block_params(frame.next_block())
                    );

                    state
                        .stack
                        .extend_from_slice(builder.block_params(frame.next_block()));
                    state.reachable = true;
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn translate_icmp(
    state: &mut State,
    builder: &mut FunctionBuilder,
    cond: IntCC,
) -> crate::Result<()> {
    let (x, y) = state.pop2()?;
    let val = builder.ins().icmp(cond, x, y);
    state.push1(builder.ins().uextend(types::I32, val));
    Ok(())
}

fn translate_fcmp(
    state: &mut State,
    builder: &mut FunctionBuilder,
    cond: FloatCC,
) -> crate::Result<()> {
    let (x, y) = state.pop2()?;
    let val = builder.ins().fcmp(cond, x, y);
    state.push1(builder.ins().uextend(types::I32, val));
    Ok(())
}

fn translate_unary_arith(
    state: &mut State,
    builder: &mut FunctionBuilder,
    opcode: Opcode,
) -> crate::Result<()> {
    let val = state.pop1()?;
    let val_ty = builder.func.dfg.value_type(val);
    let (inst, dfg) = builder.ins().Unary(opcode, val_ty, val);
    state.push1(dfg.first_result(inst));
    Ok(())
}

fn translate_binary_arith(
    state: &mut State,
    builder: &mut FunctionBuilder,
    opcode: Opcode,
) -> crate::Result<()> {
    let (x, y) = state.pop2()?;
    let val_ty = builder.func.dfg.value_type(x);
    let (inst, dfg) = builder.ins().Binary(opcode, val_ty, x, y);
    state.push1(dfg.first_result(inst));
    Ok(())
}

fn cast_index_to_pointer_ty(
    mut pos: FuncCursor,
    index: Value,
    index_ty: Type,
    pointer_ty: Type,
    pcc: bool,
) -> Value {
    if index_ty == pointer_ty {
        return index;
    }
    // Note that using 64-bit heaps on a 32-bit host is not currently supported,
    // would require at least a bounds check here to ensure that the truncation
    // from 64-to-32 bits doesn't lose any upper bits. For now though we're
    // mostly interested in the 32-bit-heaps-on-64-bit-hosts cast.
    assert!(index_ty.bits() < pointer_ty.bits());

    // Convert `index` to `addr_ty`.
    let extended_index = pos.ins().uextend(pointer_ty, index);

    // Add a range fact on the extended value.
    if pcc {
        pos.func.dfg.facts[extended_index] = Some(Fact::max_range_for_width_extended(
            u16::try_from(index_ty.bits()).unwrap(),
            u16::try_from(pointer_ty.bits()).unwrap(),
        ));
    }

    // Add debug value-label alias so that debuginfo can name the extended
    // value as the address
    let loc = pos.srcloc();
    let loc = RelSourceLoc::from_base_offset(pos.func.params.base_srcloc(), loc);
    pos.func
        .stencil
        .dfg
        .add_value_label_alias(extended_index, loc, index);

    extended_index
}

enum Reachability {
    Unreachable,
    Reachable((Value, MemFlags)),
}

/// Convert a linear memory index into a bounds-checked machine address
fn index2addr(
    builder: &mut FunctionBuilder,
    index: Value,
    heap: Heap,
    access_size: u8,
    memarg: wasmparser::MemArg,
    env: &mut dyn FuncTranslationEnvironment,
) -> crate::Result<Reachability> {
    // In a normal WASM runtime this would be the place to emit bound checking code,
    // but we rely on the fact that access to unmapped virtual memory will trigger an exception
    // this assumption is a bit flawed in the presence of multiple, possibly imported/exported memories
    // but if saves us from having to emit expensive bound checks

    let pcc = env.proof_carrying_code();
    let addr_ty = env.target_config().pointer_type();
    let index_ty = builder.func.dfg.value_type(index);
    let index = cast_index_to_pointer_ty(builder.cursor(), index, index_ty, addr_ty, pcc);

    let heap = &env.heaps()[heap];

    // optimization for when we can statically assert this access will trap
    if memarg.offset + u64::from(access_size) > heap.max_size {
        builder.ins().trap(TrapCode::HeapOutOfBounds);
        return Ok(Reachability::Unreachable);
    }

    let heap_base = builder.ins().global_value(addr_ty, heap.base);
    // emit pcc fact for heap base
    if let Some(ty) = heap.memory_type {
        builder.func.dfg.facts[heap_base] = Some(Fact::Mem {
            ty,
            min_offset: 0,
            max_offset: 0,
            nullable: false,
        });
    }

    let base_and_index = builder.ins().iadd(heap_base, index);
    // emit pcc fact for base + index
    if let Some(ty) = heap.memory_type {
        builder.func.dfg.facts[base_and_index] = Some(Fact::Mem {
            ty,
            min_offset: 0,
            max_offset: u64::from(u32::MAX),
            nullable: false,
        });
    }

    let addr = if memarg.offset == 0 {
        base_and_index
    } else {
        let offset = builder.ins().iconst(addr_ty, i64::try_from(memarg.offset)?);
        // emit pcc fact for offset
        if pcc {
            builder.func.dfg.facts[offset] = Some(Fact::constant(
                u16::try_from(addr_ty.bits()).unwrap(),
                u64::from(memarg.offset),
            ));
        }

        let base_index_and_offset = builder.ins().iconst(addr_ty, i64::try_from(memarg.offset)?);
        // emit pcc fact for base + index + offset
        if let Some(ty) = heap.memory_type {
            builder.func.dfg.facts[base_index_and_offset] = Some(Fact::Mem {
                ty,
                min_offset: u64::from(memarg.offset),
                max_offset: u64::from(u32::MAX).checked_add(memarg.offset).unwrap(),
                nullable: false,
            });
        }

        base_index_and_offset
    };

    let mut flags = MemFlags::new();
    flags.set_endianness(ir::Endianness::Little);
    flags.set_heap();

    if heap.memory_type.is_some() {
        // Proof-carrying code is enabled; check this memory access.
        flags.set_checked();
    }

    Ok(Reachability::Reachable((addr, flags)))
}

fn mem_op_size(opcode: Opcode, ty: Type) -> u8 {
    match opcode {
        Opcode::Istore8 | Opcode::Sload8 | Opcode::Uload8 => 1,
        Opcode::Istore16 | Opcode::Sload16 | Opcode::Uload16 => 2,
        Opcode::Istore32 | Opcode::Sload32 | Opcode::Uload32 => 4,
        Opcode::Store | Opcode::Load => u8::try_from(ty.bytes()).unwrap(),
        _ => panic!("unknown size of mem op for {:?}", opcode),
    }
}

fn translate_load(
    state: &mut State,
    builder: &mut FunctionBuilder,
    opcode: Opcode,
    result_ty: Type,
    memarg: wasmparser::MemArg,
    env: &mut dyn FuncTranslationEnvironment,
) -> crate::Result<()> {
    let index = state.pop1()?;
    let heap = state.get_or_make_heap(builder.func, memarg.memory, env)?;

    let access_size = mem_op_size(opcode, result_ty);

    let Reachability::Reachable((addr, flags)) =
        index2addr(builder, index, heap, access_size, memarg, env)?
    else {
        state.reachable = false;
        return Ok(());
    };

    let (load, dfg) = builder
        .ins()
        .Load(opcode, result_ty, flags, Offset32::new(0), addr);
    state.push1(dfg.first_result(load));

    Ok(())
}

fn translate_store(
    state: &mut State,
    builder: &mut FunctionBuilder,
    opcode: Opcode,
    memarg: wasmparser::MemArg,
    env: &mut dyn FuncTranslationEnvironment,
) -> crate::Result<()> {
    let val = state.pop1()?;
    let val_ty = builder.func.dfg.value_type(val);

    let index = state.pop1()?;
    let heap = state.get_or_make_heap(builder.func, memarg.memory, env)?;

    let access_size = mem_op_size(opcode, val_ty);

    let Reachability::Reachable((addr, flags)) =
        index2addr(builder, index, heap, access_size, memarg, env)?
    else {
        state.reachable = false;
        return Ok(());
    };

    builder
        .ins()
        .Store(opcode, val_ty, flags, Offset32::new(0), val, addr);

    Ok(())
}

#[derive(Debug, Clone)]
pub enum BlockTypeParamsOrReturns<'a> {
    Empty,
    One(wasmparser::ValueType),
    Many(wasmparser::VecIter<'a, wasmparser::ValueType>),
}

impl<'a> Iterator for BlockTypeParamsOrReturns<'a> {
    type Item = wasmparser::ValueType;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            BlockTypeParamsOrReturns::Empty => None,
            BlockTypeParamsOrReturns::One(ty) => {
                let ty = *ty;
                *self = Self::Empty;
                Some(ty)
            }
            BlockTypeParamsOrReturns::Many(iter) => Some(iter.next()?.ok()?),
        }
    }
}

pub fn block_type_params_returns<'a>(
    ty: &wasmparser::BlockType,
    env: &'a dyn FuncTranslationEnvironment,
) -> (BlockTypeParamsOrReturns<'a>, BlockTypeParamsOrReturns<'a>) {
    use wasmparser::BlockType;

    match ty {
        BlockType::Empty => (
            BlockTypeParamsOrReturns::Empty,
            BlockTypeParamsOrReturns::Empty,
        ),
        BlockType::Type(ty) => (
            BlockTypeParamsOrReturns::Empty,
            BlockTypeParamsOrReturns::One(*ty),
        ),
        BlockType::FunctionType(typeidx) => {
            let ty = &env.lookup_type(*typeidx);

            (
                BlockTypeParamsOrReturns::Many(ty.params().unwrap().iter()),
                BlockTypeParamsOrReturns::Many(ty.results().unwrap().iter()),
            )
        }
    }
}

fn block_with_params(
    builder: &mut FunctionBuilder,
    params: impl Iterator<Item = wasmparser::ValueType>,
    env: &dyn FuncTranslationEnvironment,
) -> ir::Block {
    use wasmparser::ValueType;

    let block = builder.create_block();
    for ty in params {
        match ty {
            ValueType::I32 => builder.append_block_param(block, types::I32),
            ValueType::I64 => builder.append_block_param(block, types::I64),
            ValueType::F32 => builder.append_block_param(block, types::F32),
            ValueType::F64 => builder.append_block_param(block, types::F64),
            ValueType::V128 => todo!("simd"),
            ValueType::FuncRef | ValueType::ExternRef => {
                builder.append_block_param(block, env.target_config().pointer_type())
            }
        };
    }
    block
}
