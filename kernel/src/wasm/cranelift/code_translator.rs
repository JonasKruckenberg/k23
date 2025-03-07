// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::util::zip_eq::IteratorExt;
use crate::wasm::cranelift::CraneliftGlobal;
use crate::wasm::cranelift::env::TranslationEnvironment;
use crate::wasm::cranelift::state::{ControlStackFrame, ElseData, FuncTranslationState};
use crate::wasm::cranelift::utils::{
    block_with_params, blocktype_params_results, f32_translation, f64_translation,
};
use crate::wasm::indices::{
    DataIndex, ElemIndex, FuncIndex, GlobalIndex, MemoryIndex, TableIndex, TypeIndex,
};
use crate::wasm::trap::{TRAP_NULL_REFERENCE, TRAP_UNREACHABLE};
use crate::wasm_unsupported;
use alloc::vec;
use alloc::vec::Vec;
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::{
    F32, F32X4, F64, F64X2, I8, I8X16, I16, I16X8, I32, I32X4, I64, I64X2,
};
use cranelift_codegen::ir::{
    AtomicRmwOp, ConstantData, JumpTableData, MemFlags, TrapCode, ValueLabel,
};
use cranelift_codegen::ir::{InstBuilder, Type, Value};
use cranelift_entity::packed_option::ReservedValue;
use cranelift_frontend::{FunctionBuilder, Variable};
use fallible_iterator::FallibleIterator;
use hashbrown::{HashMap, hash_map};
use smallvec::SmallVec;
use wasmparser::{FuncValidator, MemArg, Operator, WasmModuleResources};

/// Like `Option<T>` but specifically for passing information about transitions
/// from reachable to unreachable state and the like from callees to callers.
///
/// Marked `must_use` to force callers to update
/// `FuncTranslationState::reachable` as necessary.
#[derive(PartialEq, Eq)]
#[must_use]
pub enum Reachability<T> {
    /// The Wasm execution state is reachable, here is a `T`.
    Reachable(T),
    /// The Wasm execution state has been determined to be statically
    /// unreachable. It is the receiver of this value's responsibility to update
    /// `FuncTranslationState::reachable` as necessary.
    Unreachable,
}

/// Given a `Reachability<T>`, unwrap the inner `T` or, when unreachable, set
/// `state.reachable = false` and return.
///
/// Used in combination with calling `prepare_addr` and `prepare_atomic_addr`
/// when we can statically determine that a Wasm access will unconditionally
/// trap.
macro_rules! unwrap_or_return_unreachable_state {
    ($state:ident, $value:expr) => {
        match $value {
            Reachability::Reachable(x) => x,
            Reachability::Unreachable => {
                $state.reachable = false;
                return Ok(());
            }
        }
    };
}

/// Translates wasm operators into Cranelift IR instructions.
#[expect(clippy::too_many_lines, reason = "This is the big match statement")]
pub fn translate_operator(
    validator: &mut FuncValidator<impl WasmModuleResources>,
    op: &Operator,
    builder: &mut FunctionBuilder,
    state: &mut FuncTranslationState,
    env: &mut TranslationEnvironment,
) -> crate::wasm::Result<()> {
    if !state.reachable {
        translate_unreachable_operator(validator, op, builder, state, env);
        return Ok(());
    }

    // Given that we believe the current block is reachable, the FunctionBuilder ought to agree.
    debug_assert!(!builder.is_unreachable());

    match op {
        Operator::Unreachable => {
            builder.ins().trap(TRAP_UNREACHABLE);
            state.reachable = false;
        }
        Operator::Nop => {
            // We do nothing
        }
        Operator::Drop => {
            state.pop1();
        }
        Operator::Select => {
            let (mut arg1, mut arg2, cond) = state.pop3();
            if builder.func.dfg.value_type(arg1).is_vector() {
                arg1 = optionally_bitcast_vector(arg1, I8X16, builder);
            }
            if builder.func.dfg.value_type(arg2).is_vector() {
                arg2 = optionally_bitcast_vector(arg2, I8X16, builder);
            }
            state.push1(builder.ins().select(cond, arg1, arg2));
        }
        /***************************** Control flow blocks **********************************
         *  When starting a control flow block, we create a new `Block` that will hold the code
         *  after the block, and we push a frame on the control stack. Depending on the type
         *  of block, we create a new `Block` for the body of the block with an associated
         *  jump instruction.
         *
         *  The `End` instruction pops the last control frame from the control stack, seals
         *  the destination block (since `br` instructions targeting it only appear inside the
         *  block and have already been translated) and modify the value stack to use the
         *  possible `Block`'s arguments values.
         ***********************************************************************************/
        Operator::Block { blockty } => {
            let (params, results) = blocktype_params_results(validator, *blockty);
            let next = block_with_params(builder, results.clone(), env);
            state.push_block(next, params.len(), results.len());
        }
        Operator::Loop { blockty } => {
            let (params, results) = blocktype_params_results(validator, *blockty);
            let loop_body = block_with_params(builder, params.clone(), env);
            let next = block_with_params(builder, results.clone(), env);
            canonicalise_then_jump(builder, loop_body, state.peekn(params.len()));
            state.push_loop(loop_body, next, params.len(), results.len());

            // Pop the initial `Block` actuals and replace them with the `Block`'s
            // params since control flow joins at the top of the loop.
            state.popn(params.len());
            state
                .stack
                .extend_from_slice(builder.block_params(loop_body));

            builder.switch_to_block(loop_body);
            // env.before_loop_header(builder)?;
        }
        Operator::If { blockty } => {
            let val = state.pop1();

            let next_block = builder.create_block();
            let (params, results) = blocktype_params_results(validator, *blockty);
            let (destination, else_data) = if params.clone().eq(results.clone()) {
                // It is possible there is no `else` block, so we will only
                // allocate a block for it if/when we find the `else`. For now,
                // we if the condition isn't true, then we jump directly to the
                // destination block following the whole `if...end`. If we do end
                // up discovering an `else`, then we will allocate a block for it
                // and go back and patch the jump.
                let destination = block_with_params(builder, results.clone(), env);
                let branch_inst = canonicalise_brif(
                    builder,
                    val,
                    next_block,
                    &[],
                    destination,
                    state.peekn(params.len()),
                );
                (
                    destination,
                    ElseData::NoElse {
                        branch_inst,
                        placeholder: destination,
                    },
                )
            } else {
                // The `if` type signature is not valid without an `else` block,
                // so we eagerly allocate the `else` block here.
                let destination = block_with_params(builder, results.clone(), env);
                let else_block = block_with_params(builder, params.clone(), env);
                canonicalise_brif(
                    builder,
                    val,
                    next_block,
                    &[],
                    else_block,
                    state.peekn(params.len()),
                );
                builder.seal_block(else_block);
                (destination, ElseData::WithElse { else_block })
            };

            builder.seal_block(next_block); // Only predecessor is the current block.
            builder.switch_to_block(next_block);

            // Here we append an argument to a Block targeted by an argumentless jump instruction
            // But in fact there are two cases:
            // - either the If does not have a Else clause, in that case ty = EmptyBlock
            //   and we add nothing;
            // - either the If have an Else clause, in that case the destination of this jump
            //   instruction will be changed later when we parse the Else operator.
            state.push_if(
                destination,
                else_data,
                params.len(),
                results.len(),
                *blockty,
            );
        }
        Operator::Else => {
            let i = state.control_stack.len().checked_sub(1).unwrap();
            match *state.control_stack.get_mut(i).unwrap() {
                ControlStackFrame::If {
                    ref else_data,
                    head_is_reachable,
                    ref mut consequent_ends_reachable,
                    num_return_values,
                    blocktype,
                    destination,
                    ..
                } => {
                    // We finished the consequent, so record its final
                    // reachability state.
                    debug_assert!(consequent_ends_reachable.is_none());
                    *consequent_ends_reachable = Some(state.reachable);

                    if head_is_reachable {
                        // We have a branch from the head of the `if` to the `else`.
                        state.reachable = true;

                        // Ensure we have a block for the `else` block (it may have
                        // already been pre-allocated, see `ElseData` for details).
                        let else_block = match *else_data {
                            ElseData::NoElse {
                                branch_inst,
                                placeholder,
                            } => {
                                let (params, _results) =
                                    blocktype_params_results(validator, blocktype);
                                debug_assert_eq!(params.len(), num_return_values);
                                let else_block = block_with_params(builder, params.clone(), env);
                                canonicalise_then_jump(
                                    builder,
                                    destination,
                                    state.peekn(params.len()),
                                );
                                state.popn(params.len());

                                builder.change_jump_destination(
                                    branch_inst,
                                    placeholder,
                                    else_block,
                                );
                                builder.seal_block(else_block);
                                else_block
                            }
                            ElseData::WithElse { else_block } => {
                                canonicalise_then_jump(
                                    builder,
                                    destination,
                                    state.peekn(num_return_values),
                                );
                                state.popn(num_return_values);
                                else_block
                            }
                        };

                        // You might be expecting that we push the parameters for this
                        // `else` block here, something like this:
                        //
                        //     state.pushn(&control_stack_frame.params);
                        //
                        // We don't do that because they are already on the top of the stack
                        // for us: we pushed the parameters twice when we saw the initial
                        // `if` so that we wouldn't have to save the parameters in the
                        // `ControlStackFrame` as another `Vec` allocation.

                        builder.switch_to_block(else_block);

                        // We don't bother updating the control frame's `ElseData`
                        // to `WithElse` because nothing else will read it.
                    }
                }
                _ => unreachable!(),
            }
        }
        Operator::End => {
            let frame = state.control_stack.pop().unwrap();
            let next_block = frame.following_code();
            let return_count = frame.num_return_values();
            let return_args = state.peekn_mut(return_count);

            canonicalise_then_jump(builder, next_block, return_args);
            // You might expect that if we just finished an `if` block that
            // didn't have a corresponding `else` block, then we would clean
            // up our duplicate set of parameters that we pushed earlier
            // right here. However, we don't have to explicitly do that,
            // since we truncate the stack back to the original height
            // below.

            builder.switch_to_block(next_block);
            builder.seal_block(next_block);

            // If it is a loop we also have to seal the body loop block
            if let ControlStackFrame::Loop { header, .. } = frame {
                builder.seal_block(header);
            }

            frame.truncate_value_stack_to_original_size(&mut state.stack);
            state
                .stack
                .extend_from_slice(builder.block_params(next_block));
        }
        /**************************** Branch instructions *********************************
         * The branch instructions all have as arguments a target nesting level, which
         * corresponds to how many control stack frames do we have to pop to get the
         * destination `Block`.
         *
         * Once the destination `Block` is found, we sometimes have to declare a certain depth
         * of the stack unreachable, because some branch instructions are terminator.
         *
         * The `br_table` case is much more complicated because Cranelift's `br_table` instruction
         * does not support jump arguments like all the other branch instructions. That is why, in
         * the case where we would use jump arguments for every other branch instruction, we
         * need to split the critical edges leaving the `br_tables` by creating one `Block` per
         * table destination; the `br_table` will point to these newly created `Blocks` and these
         * `Block`s contain only a jump instruction pointing to the final destination, this time with
         * jump arguments.
         *
         * This system is also implemented in Cranelift's SSA construction algorithm, because
         * `use_var` located in a destination `Block` of a `br_table` might trigger the addition
         * of jump arguments in each predecessor branch instruction, one of which might be a
         * `br_table`.
         ***********************************************************************************/
        Operator::Br { relative_depth } => {
            // FIXME wow this is ugly
            let i = state
                .control_stack
                .len()
                .checked_sub(1)
                .unwrap()
                .checked_sub(usize::try_from(*relative_depth).unwrap())
                .unwrap();

            let (return_count, br_destination) = {
                let frame = &mut state.control_stack[i];
                // We signal that all the code that follows until the next End is unreachable
                frame.set_branched_to_exit();
                let return_count = if frame.is_loop() {
                    frame.num_param_values()
                } else {
                    frame.num_return_values()
                };
                (return_count, frame.br_destination())
            };
            let destination_args = state.peekn_mut(return_count);
            canonicalise_then_jump(builder, br_destination, destination_args);
            state.popn(return_count);
            state.reachable = false;
        }
        Operator::BrIf { relative_depth } => translate_br_if(*relative_depth, builder, state),
        Operator::BrTable { targets } => {
            let default = targets.default();
            let mut min_depth = default;
            for depth in targets.targets() {
                let depth = depth?;
                if depth < min_depth {
                    min_depth = depth;
                }
            }
            let jump_args_count = {
                // FIXME wow this is ugly
                let i = state
                    .control_stack
                    .len()
                    .checked_sub(1)
                    .unwrap()
                    .checked_sub(usize::try_from(min_depth).unwrap())
                    .unwrap();

                let min_depth_frame = &state.control_stack[i];
                if min_depth_frame.is_loop() {
                    min_depth_frame.num_param_values()
                } else {
                    min_depth_frame.num_return_values()
                }
            };
            let val = state.pop1();
            let mut data = Vec::with_capacity(targets.len() as usize);
            if jump_args_count == 0 {
                // No jump arguments
                for depth in targets.targets() {
                    let depth = depth?;
                    let block = {
                        // FIXME wow this is ugly
                        let i = state
                            .control_stack
                            .len()
                            .checked_sub(1)
                            .unwrap()
                            .checked_sub(usize::try_from(depth).unwrap())
                            .unwrap();

                        let frame = &mut state.control_stack[i];
                        frame.set_branched_to_exit();
                        frame.br_destination()
                    };
                    data.push(builder.func.dfg.block_call(block, &[]));
                }
                let block = {
                    // FIXME wow this is ugly
                    let i = state
                        .control_stack
                        .len()
                        .checked_sub(1)
                        .unwrap()
                        .checked_sub(usize::try_from(default).unwrap())
                        .unwrap();

                    let frame = &mut state.control_stack[i];
                    frame.set_branched_to_exit();
                    frame.br_destination()
                };
                let block = builder.func.dfg.block_call(block, &[]);
                let jt = builder.create_jump_table(JumpTableData::new(block, &data));
                builder.ins().br_table(val, jt);
            } else {
                // Here we have jump arguments, but Cranelift's br_table doesn't support them
                // We then proceed to split the edges going out of the br_table
                let return_count = jump_args_count;
                let mut dest_block_sequence = vec![];
                let mut dest_block_map = HashMap::new();
                for depth in targets.targets() {
                    let depth = depth?;
                    let branch_block = match dest_block_map.entry(depth as usize) {
                        hash_map::Entry::Occupied(entry) => *entry.get(),
                        hash_map::Entry::Vacant(entry) => {
                            let block = builder.create_block();
                            dest_block_sequence.push((depth as usize, block));
                            *entry.insert(block)
                        }
                    };
                    data.push(builder.func.dfg.block_call(branch_block, &[]));
                }
                let default_branch_block = match dest_block_map.entry(default as usize) {
                    hash_map::Entry::Occupied(entry) => *entry.get(),
                    hash_map::Entry::Vacant(entry) => {
                        let block = builder.create_block();
                        dest_block_sequence.push((default as usize, block));
                        *entry.insert(block)
                    }
                };
                let default_branch_block = builder.func.dfg.block_call(default_branch_block, &[]);
                let jt = builder.create_jump_table(JumpTableData::new(default_branch_block, &data));
                builder.ins().br_table(val, jt);
                for (depth, dest_block) in dest_block_sequence {
                    builder.switch_to_block(dest_block);
                    builder.seal_block(dest_block);
                    let real_dest_block = {
                        // FIXME wow this is ugly
                        let i = state
                            .control_stack
                            .len()
                            .checked_sub(1)
                            .unwrap()
                            .checked_sub(depth)
                            .unwrap();

                        let frame = &mut state.control_stack[i];
                        frame.set_branched_to_exit();
                        frame.br_destination()
                    };
                    let destination_args = state.peekn_mut(return_count);
                    canonicalise_then_jump(builder, real_dest_block, destination_args);
                }
                state.popn(return_count);
            }
            state.reachable = false;
        }
        Operator::Return => {
            let return_count = {
                let frame = &mut state.control_stack[0];
                frame.num_return_values()
            };
            {
                let return_args = state.peekn_mut(return_count);
                // env.handle_before_return(&return_args, builder)?;
                bitcast_wasm_returns(return_args, builder, env);
                builder.ins().return_(return_args);
            }
            state.popn(return_count);
            state.reachable = false;
        }
        /************************************ Calls ****************************************
         * The call instructions pop off their arguments from the stack and append their
         * return values to it. `call_indirect` needs environment support because there is an
         * argument referring to an index in the external functions table of the module.
         ************************************************************************************/
        Operator::Call { function_index } => {
            let function_index = FuncIndex::from_u32(*function_index);
            let (fref, num_args) = state.get_direct_func(builder.func, function_index, env);

            // Bitcast any vector arguments to their default type, I8X16, before calling.
            let args = state.peekn_mut(num_args);
            bitcast_wasm_params(
                builder.func.dfg.ext_funcs[fref].signature,
                args,
                builder,
                env,
            );

            let call = env.translate_call(builder, function_index, fref, args);
            let inst_results = builder.inst_results(call);
            debug_assert_eq!(
                inst_results.len(),
                builder.func.dfg.signatures[builder.func.dfg.ext_funcs[fref].signature]
                    .returns
                    .len(),
                "translate_call results should match the call signature"
            );
            state.popn(num_args);
            state.pushn(inst_results);
        }
        Operator::CallIndirect {
            type_index,
            table_index,
        } => {
            let type_index = TypeIndex::from_u32(*type_index);
            // `type_index` is the index of the function's signature and
            // `table_index` is the index of the table to search the function
            // in.
            let (sigref, num_args) = state.get_indirect_sig(builder.func, type_index, env);
            let callee = state.pop1();

            // Bitcast any vector arguments to their default type, I8X16, before calling.
            let args = state.peekn_mut(num_args);
            bitcast_wasm_params(sigref, args, builder, env);

            let table_index = TableIndex::from_u32(*table_index);
            let table = state.get_table(builder.func, table_index, env).clone();

            let call = unwrap_or_return_unreachable_state!(state, {
                env.translate_call_indirect(
                    builder,
                    table_index,
                    &table,
                    type_index,
                    sigref,
                    callee,
                    state.peekn(num_args),
                )
            });

            let inst_results = builder.inst_results(call);
            debug_assert_eq!(
                inst_results.len(),
                builder.func.dfg.signatures[sigref].returns.len(),
                "translate_call_indirect results should match the call signature"
            );
            state.popn(num_args);
            state.pushn(inst_results);
        }
        /******************************* Tail Calls ******************************************
         * The tail call instructions pop their arguments from the stack and
         * then permanently transfer control to their callee. The indirect
         * version requires environment support (while the direct version can
         * optionally be hooked but doesn't require it) it interacts with the
         * VM's runtime state via tables.
         ************************************************************************************/
        Operator::ReturnCall { function_index } => {
            let function_index = FuncIndex::from_u32(*function_index);
            let (fref, num_args) = state.get_direct_func(builder.func, function_index, env);

            // Bitcast any vector arguments to their default type, I8X16, before calling.
            let args = state.peekn_mut(num_args);
            bitcast_wasm_params(
                builder.func.dfg.ext_funcs[fref].signature,
                args,
                builder,
                env,
            );

            env.translate_return_call(builder, function_index, fref, args)?;

            state.popn(num_args);
            state.reachable = false;
        }
        Operator::ReturnCallIndirect {
            type_index,
            table_index,
        } => {
            let type_index = TypeIndex::from_u32(*type_index);
            // `type_index` is the index of the function's signature and
            // `table_index` is the index of the table to search the function
            // in.
            let (sigref, num_args) = state.get_indirect_sig(builder.func, type_index, env);
            let callee = state.pop1();

            // Bitcast any vector arguments to their default type, I8X16, before calling.
            let args = state.peekn_mut(num_args);
            bitcast_wasm_params(sigref, args, builder, env);

            env.translate_return_call_indirect(
                builder,
                TableIndex::from_u32(*table_index),
                type_index,
                sigref,
                callee,
                state.peekn(num_args),
            )?;

            state.popn(num_args);
            state.reachable = false;
        }
        /********************************** Locals ****************************************
         *  `get_local` and `set_local` are treated as non-SSA variables and will completely
         *  disappear in the Cranelift Code
         ***********************************************************************************/
        Operator::LocalGet { local_index } => {
            let val = builder.use_var(Variable::from_u32(*local_index));
            state.push1(val);
            let label = ValueLabel::from_u32(*local_index);
            builder.set_val_label(val, label);
        }
        Operator::LocalSet { local_index } => {
            let mut val = state.pop1();

            // Ensure SIMD values are cast to their default Cranelift type, I8x16.
            let ty = builder.func.dfg.value_type(val);
            if ty.is_vector() {
                val = optionally_bitcast_vector(val, I8X16, builder);
            }

            builder.def_var(Variable::from_u32(*local_index), val);
            let label = ValueLabel::from_u32(*local_index);
            builder.set_val_label(val, label);
        }
        Operator::LocalTee { local_index } => {
            let mut val = state.peek1();

            // Ensure SIMD values are cast to their default Cranelift type, I8x16.
            let ty = builder.func.dfg.value_type(val);
            if ty.is_vector() {
                val = optionally_bitcast_vector(val, I8X16, builder);
            }

            builder.def_var(Variable::from_u32(*local_index), val);
            let label = ValueLabel::from_u32(*local_index);
            builder.set_val_label(val, label);
        }
        /********************************** Globals ****************************************
         *  `get_global` and `set_global` are handled by the environment.
         ***********************************************************************************/
        Operator::GlobalGet { global_index } => {
            let global_index = GlobalIndex::from_u32(*global_index);
            let val = match *state.get_global(builder.func, global_index, env) {
                CraneliftGlobal::Const(val) => val,
                CraneliftGlobal::Memory { gv, offset, ty } => {
                    let addr = builder.ins().global_value(env.pointer_type(), gv);
                    let mut flags = MemFlags::trusted();
                    // Put globals in the "table" abstract mem category as well.
                    flags.set_alias_region(Some(ir::AliasRegion::Table));
                    builder.ins().load(ty, flags, addr, offset)
                }
                CraneliftGlobal::Custom => {
                    env.translate_custom_global_get(builder, global_index)?
                }
            };
            state.push1(val);
        }
        Operator::GlobalSet { global_index } => {
            let global_index = GlobalIndex::from_u32(*global_index);
            match *state.get_global(builder.func, global_index, env) {
                CraneliftGlobal::Const(_) => panic!("global #{global_index:?} is a constant"),
                CraneliftGlobal::Memory { gv, offset, ty } => {
                    let addr = builder.ins().global_value(env.pointer_type(), gv);
                    let mut flags = MemFlags::trusted();
                    // Put globals in the "table" abstract mem category as well.
                    flags.set_alias_region(Some(ir::AliasRegion::Table));
                    let mut val = state.pop1();
                    // Ensure SIMD values are cast to their default Cranelift type, I8x16.
                    if ty.is_vector() {
                        val = optionally_bitcast_vector(val, I8X16, builder);
                    }
                    debug_assert_eq!(ty, builder.func.dfg.value_type(val));
                    builder.ins().store(flags, val, addr, offset);
                }
                CraneliftGlobal::Custom => {
                    let val = state.pop1();
                    env.translate_custom_global_set(builder, global_index, val)?;
                }
            }
        }
        /******************************* Memory management ***********************************
         * Memory management is handled by environment. It is usually translated into calls to
         * special functions.
         ************************************************************************************/
        Operator::MemoryGrow { mem } => {
            // The WebAssembly MVP only supports one linear memory, but we expect the reserved
            // argument to be a memory index.
            let mem_index = MemoryIndex::from_u32(*mem);
            let delta = state.pop1();
            // env.before_memory_grow(builder, delta, mem_index)?;
            state.push1(env.translate_memory_grow(builder.cursor(), mem_index, delta)?);
        }
        Operator::MemorySize { mem } => {
            let mem_index = MemoryIndex::from_u32(*mem);
            state.push1(env.translate_memory_size(builder.cursor(), mem_index)?);
        }

        Operator::I32Const { value } => {
            state.push1(builder.ins().iconst(I32, i64::from(*value)));
        }
        Operator::I64Const { value } => state.push1(builder.ins().iconst(I64, *value)),
        Operator::F32Const { value } => {
            state.push1(builder.ins().f32const(f32_translation(*value)));
        }
        Operator::F64Const { value } => {
            state.push1(builder.ins().f64const(f64_translation(*value)));
        }

        // integer operators
        Operator::I32Add | Operator::I64Add => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().iadd(arg1, arg2));
        }
        Operator::I32Sub | Operator::I64Sub => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().isub(arg1, arg2));
        }
        Operator::I32Mul | Operator::I64Mul => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().imul(arg1, arg2));
        }
        Operator::I32DivS | Operator::I64DivS => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().sdiv(arg1, arg2));
        }
        Operator::I32DivU | Operator::I64DivU => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().udiv(arg1, arg2));
        }
        Operator::I32RemS | Operator::I64RemS => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().srem(arg1, arg2));
        }
        Operator::I32RemU | Operator::I64RemU => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().urem(arg1, arg2));
        }
        Operator::I32And | Operator::I64And => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().band(arg1, arg2));
        }
        Operator::I32Or | Operator::I64Or => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().bor(arg1, arg2));
        }
        Operator::I32Xor | Operator::I64Xor => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().bxor(arg1, arg2));
        }
        Operator::I32Shl | Operator::I64Shl => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().ishl(arg1, arg2));
        }
        Operator::I32ShrS | Operator::I64ShrS => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().sshr(arg1, arg2));
        }
        Operator::I32ShrU | Operator::I64ShrU => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().ushr(arg1, arg2));
        }
        Operator::I32Rotl | Operator::I64Rotl => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().rotl(arg1, arg2));
        }
        Operator::I32Rotr | Operator::I64Rotr => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().rotr(arg1, arg2));
        }
        Operator::I32Clz | Operator::I64Clz => {
            let arg = state.pop1();
            state.push1(builder.ins().clz(arg));
        }
        Operator::I32Ctz | Operator::I64Ctz => {
            let arg = state.pop1();
            state.push1(builder.ins().ctz(arg));
        }
        Operator::I32Popcnt | Operator::I64Popcnt => {
            let arg = state.pop1();
            state.push1(builder.ins().popcnt(arg));
        }
        Operator::I32WrapI64 => {
            let val = state.pop1();
            state.push1(builder.ins().ireduce(I32, val));
        }
        Operator::I64ExtendI32S => {
            let val = state.pop1();
            state.push1(builder.ins().sextend(I64, val));
        }
        Operator::I64ExtendI32U => {
            let val = state.pop1();
            state.push1(builder.ins().uextend(I64, val));
        }

        // floating-point operators
        Operator::F32Add | Operator::F64Add => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().fadd(arg1, arg2));
        }
        Operator::F32Sub | Operator::F64Sub => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().fsub(arg1, arg2));
        }
        Operator::F32Mul | Operator::F64Mul => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().fmul(arg1, arg2));
        }
        Operator::F32Div | Operator::F64Div => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().fdiv(arg1, arg2));
        }
        Operator::F32Min | Operator::F64Min => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().fmin(arg1, arg2));
        }
        Operator::F32Max | Operator::F64Max => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().fmax(arg1, arg2));
        }
        Operator::F32Copysign | Operator::F64Copysign => {
            let (arg1, arg2) = state.pop2();
            state.push1(builder.ins().fcopysign(arg1, arg2));
        }
        Operator::F32Sqrt | Operator::F64Sqrt => {
            let arg = state.pop1();
            state.push1(builder.ins().sqrt(arg));
        }
        Operator::F32Ceil | Operator::F64Ceil => {
            let arg = state.pop1();
            state.push1(builder.ins().ceil(arg));
        }
        Operator::F32Floor | Operator::F64Floor => {
            let arg = state.pop1();
            state.push1(builder.ins().floor(arg));
        }
        Operator::F32Trunc | Operator::F64Trunc => {
            let arg = state.pop1();
            state.push1(builder.ins().trunc(arg));
        }
        Operator::F32Nearest | Operator::F64Nearest => {
            let arg = state.pop1();
            state.push1(builder.ins().nearest(arg));
        }
        Operator::F32Abs | Operator::F64Abs => {
            let val = state.pop1();
            state.push1(builder.ins().fabs(val));
        }
        Operator::F32Neg | Operator::F64Neg => {
            let arg = state.pop1();
            state.push1(builder.ins().fneg(arg));
        }
        Operator::I32TruncF64S | Operator::I32TruncF32S => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_to_sint(I32, val));
        }
        Operator::I32TruncF64U | Operator::I32TruncF32U => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_to_uint(I32, val));
        }
        Operator::I64TruncF64U | Operator::I64TruncF32U => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_to_uint(I64, val));
        }
        Operator::I64TruncF64S | Operator::I64TruncF32S => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_to_sint(I64, val));
        }
        Operator::F32DemoteF64 => {
            let val = state.pop1();
            state.push1(builder.ins().fdemote(F32, val));
        }
        Operator::F32ConvertI64S | Operator::F32ConvertI32S => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_from_sint(F32, val));
        }
        Operator::F32ConvertI64U | Operator::F32ConvertI32U => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_from_uint(F32, val));
        }
        Operator::F64ConvertI64S | Operator::F64ConvertI32S => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_from_sint(F64, val));
        }
        Operator::F64ConvertI64U | Operator::F64ConvertI32U => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_from_uint(F64, val));
        }
        Operator::F64PromoteF32 => {
            let val = state.pop1();
            state.push1(builder.ins().fpromote(F64, val));
        }
        Operator::F32ReinterpretI32 => {
            let val = state.pop1();
            state.push1(builder.ins().bitcast(F32, MemFlags::new(), val));
        }
        Operator::F64ReinterpretI64 => {
            let val = state.pop1();
            state.push1(builder.ins().bitcast(F64, MemFlags::new(), val));
        }
        Operator::I32ReinterpretF32 => {
            let val = state.pop1();
            state.push1(builder.ins().bitcast(I32, MemFlags::new(), val));
        }
        Operator::I64ReinterpretF64 => {
            let val = state.pop1();
            state.push1(builder.ins().bitcast(I64, MemFlags::new(), val));
        }

        // comparison operators
        Operator::I32LtS | Operator::I64LtS => {
            translate_icmp(IntCC::SignedLessThan, builder, state);
        }
        Operator::I32LtU | Operator::I64LtU => {
            translate_icmp(IntCC::UnsignedLessThan, builder, state);
        }
        Operator::I32LeS | Operator::I64LeS => {
            translate_icmp(IntCC::SignedLessThanOrEqual, builder, state);
        }
        Operator::I32LeU | Operator::I64LeU => {
            translate_icmp(IntCC::UnsignedLessThanOrEqual, builder, state);
        }
        Operator::I32GtS | Operator::I64GtS => {
            translate_icmp(IntCC::SignedGreaterThan, builder, state);
        }
        Operator::I32GtU | Operator::I64GtU => {
            translate_icmp(IntCC::UnsignedGreaterThan, builder, state);
        }
        Operator::I32GeS | Operator::I64GeS => {
            translate_icmp(IntCC::SignedGreaterThanOrEqual, builder, state);
        }
        Operator::I32GeU | Operator::I64GeU => {
            translate_icmp(IntCC::UnsignedGreaterThanOrEqual, builder, state);
        }
        Operator::I32Eqz | Operator::I64Eqz => {
            let arg = state.pop1();
            let val = builder.ins().icmp_imm(IntCC::Equal, arg, 0);
            state.push1(builder.ins().uextend(I32, val));
        }
        Operator::I32Eq | Operator::I64Eq => translate_icmp(IntCC::Equal, builder, state),
        Operator::F32Eq | Operator::F64Eq => translate_fcmp(FloatCC::Equal, builder, state),
        Operator::I32Ne | Operator::I64Ne => translate_icmp(IntCC::NotEqual, builder, state),
        Operator::F32Ne | Operator::F64Ne => translate_fcmp(FloatCC::NotEqual, builder, state),
        Operator::F32Gt | Operator::F64Gt => translate_fcmp(FloatCC::GreaterThan, builder, state),
        Operator::F32Ge | Operator::F64Ge => {
            translate_fcmp(FloatCC::GreaterThanOrEqual, builder, state);
        }
        Operator::F32Lt | Operator::F64Lt => translate_fcmp(FloatCC::LessThan, builder, state),
        Operator::F32Le | Operator::F64Le => {
            translate_fcmp(FloatCC::LessThanOrEqual, builder, state);
        }

        /******************************* Load instructions ***********************************
         * Wasm specifies an integer alignment flag, but we drop it in Cranelift.
         * The memory base address is provided by the environment.
         ************************************************************************************/
        Operator::I32Load8U { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Uload8, I32, builder, state, env)
            );
        }
        Operator::I32Load16U { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Uload16, I32, builder, state, env)
            );
        }
        Operator::I32Load8S { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Sload8, I32, builder, state, env)
            );
        }
        Operator::I32Load16S { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Sload16, I32, builder, state, env)
            );
        }
        Operator::I64Load8U { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Uload8, I64, builder, state, env)
            );
        }
        Operator::I64Load16U { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Uload16, I64, builder, state, env)
            );
        }
        Operator::I64Load8S { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Sload8, I64, builder, state, env)
            );
        }
        Operator::I64Load16S { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Sload16, I64, builder, state, env)
            );
        }
        Operator::I64Load32S { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Sload32, I64, builder, state, env)
            );
        }
        Operator::I64Load32U { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Uload32, I64, builder, state, env)
            );
        }
        Operator::I32Load { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Load, I32, builder, state, env)
            );
        }
        Operator::F32Load { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Load, F32, builder, state, env)
            );
        }
        Operator::I64Load { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Load, I64, builder, state, env)
            );
        }
        Operator::F64Load { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Load, F64, builder, state, env)
            );
        }

        /****************************** Store instructions ***********************************
         * Wasm specifies an integer alignment flag but we drop it in Cranelift.
         * The memory base address is provided by the environment.
         ************************************************************************************/
        Operator::I32Store { memarg }
        | Operator::I64Store { memarg }
        | Operator::F32Store { memarg }
        | Operator::F64Store { memarg } => {
            translate_store(memarg, ir::Opcode::Store, builder, state, env)?;
        }
        Operator::I32Store8 { memarg } | Operator::I64Store8 { memarg } => {
            translate_store(memarg, ir::Opcode::Istore8, builder, state, env)?;
        }
        Operator::I32Store16 { memarg } | Operator::I64Store16 { memarg } => {
            translate_store(memarg, ir::Opcode::Istore16, builder, state, env)?;
        }
        Operator::I64Store32 { memarg } => {
            translate_store(memarg, ir::Opcode::Istore32, builder, state, env)?;
        }

        // Sign-extension
        // https://github.com/WebAssembly/sign-extension-ops
        Operator::I32Extend8S => {
            let val = state.pop1();
            state.push1(builder.ins().ireduce(I8, val));
            let val = state.pop1();
            state.push1(builder.ins().sextend(I32, val));
        }
        Operator::I32Extend16S => {
            let val = state.pop1();
            state.push1(builder.ins().ireduce(I16, val));
            let val = state.pop1();
            state.push1(builder.ins().sextend(I32, val));
        }
        Operator::I64Extend8S => {
            let val = state.pop1();
            state.push1(builder.ins().ireduce(I8, val));
            let val = state.pop1();
            state.push1(builder.ins().sextend(I64, val));
        }
        Operator::I64Extend16S => {
            let val = state.pop1();
            state.push1(builder.ins().ireduce(I16, val));
            let val = state.pop1();
            state.push1(builder.ins().sextend(I64, val));
        }
        Operator::I64Extend32S => {
            let val = state.pop1();
            state.push1(builder.ins().ireduce(I32, val));
            let val = state.pop1();
            state.push1(builder.ins().sextend(I64, val));
        }

        // Non-trapping Float-to-int Conversions
        // https://github.com/WebAssembly/nontrapping-float-to-int-conversions
        Operator::I32TruncSatF64S | Operator::I32TruncSatF32S => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_to_sint_sat(I32, val));
        }
        Operator::I32TruncSatF64U | Operator::I32TruncSatF32U => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_to_uint_sat(I32, val));
        }
        Operator::I64TruncSatF64S | Operator::I64TruncSatF32S => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_to_sint_sat(I64, val));
        }
        Operator::I64TruncSatF64U | Operator::I64TruncSatF32U => {
            let val = state.pop1();
            state.push1(builder.ins().fcvt_to_uint_sat(I64, val));
        }

        // reference-types
        // https://github.com/WebAssembly/reference-types
        Operator::TableFill { table } => {
            let table_index = TableIndex::from_u32(*table);
            let len = state.pop1();
            let val = state.pop1();
            let dest = state.pop1();
            env.translate_table_fill(builder.cursor(), table_index, dest, val, len)?;
        }
        Operator::TableGet { table: index } => {
            let table_index = TableIndex::from_u32(*index);
            let index = state.pop1();
            state.push1(env.translate_table_get(builder.cursor(), table_index, index)?);
        }
        Operator::TableSet { table: index } => {
            let table_index = TableIndex::from_u32(*index);
            let value = state.pop1();
            let index = state.pop1();
            env.translate_table_set(builder.cursor(), table_index, value, index)?;
        }
        Operator::TableGrow { table: index } => {
            let table_index = TableIndex::from_u32(*index);
            let delta = state.pop1();
            let init_value = state.pop1();
            state.push1(env.translate_table_grow(
                builder.cursor(),
                table_index,
                delta,
                init_value,
            )?);
        }
        Operator::TableSize { table: index } => {
            state.push1(env.translate_table_size(builder.cursor(), TableIndex::from_u32(*index))?);
        }
        Operator::RefNull { hty } => {
            let hty = env.convert_heap_type(*hty);
            state.push1(env.translate_ref_null(builder.cursor(), &hty));
        }
        Operator::RefIsNull => {
            let value = state.pop1();
            state.push1(env.translate_ref_is_null(builder.cursor(), value));
        }
        Operator::RefFunc { function_index } => {
            let index = FuncIndex::from_u32(*function_index);
            state.push1(env.translate_ref_func(builder.cursor(), index)?);
        }
        Operator::TypedSelect { ty: _ } => {
            // We ignore the explicit type parameter as it is only needed for
            // validation, which we require to have been performed before
            // translation.
            let (mut arg1, mut arg2, cond) = state.pop3();
            if builder.func.dfg.value_type(arg1).is_vector() {
                arg1 = optionally_bitcast_vector(arg1, I8X16, builder);
            }
            if builder.func.dfg.value_type(arg2).is_vector() {
                arg2 = optionally_bitcast_vector(arg2, I8X16, builder);
            }
            state.push1(builder.ins().select(cond, arg1, arg2));
        }
        //
        // // bulk memory operations
        // // https://github.com/WebAssembly/bulk-memory-operations
        Operator::MemoryInit { data_index, mem } => {
            let mem_index = MemoryIndex::from_u32(*mem);
            let len = state.pop1();
            let src = state.pop1();
            let dest = state.pop1();
            env.translate_memory_init(
                builder.cursor(),
                mem_index,
                DataIndex::from_u32(*data_index),
                dest,
                src,
                len,
            )?;
        }
        Operator::MemoryCopy { src_mem, dst_mem } => {
            let src_index = MemoryIndex::from_u32(*src_mem);
            let dst_index = MemoryIndex::from_u32(*dst_mem);
            let len = state.pop1();
            let src_pos = state.pop1();
            let dst_pos = state.pop1();
            env.translate_memory_copy(
                builder.cursor(),
                src_index,
                dst_index,
                src_pos,
                dst_pos,
                len,
            )?;
        }
        Operator::MemoryFill { mem } => {
            let mem_index = MemoryIndex::from_u32(*mem);
            let len = state.pop1();
            let val = state.pop1();
            let dest = state.pop1();
            env.translate_memory_fill(builder.cursor(), mem_index, dest, val, len)?;
        }
        Operator::DataDrop { data_index } => {
            env.translate_data_drop(builder.cursor(), DataIndex::from_u32(*data_index))?;
        }
        Operator::TableInit {
            elem_index,
            table: table_index,
        } => {
            let len = state.pop1();
            let src = state.pop1();
            let dest = state.pop1();
            env.translate_table_init(
                builder.cursor(),
                TableIndex::from_u32(*table_index),
                ElemIndex::from_u32(*elem_index),
                dest,
                src,
                len,
            )?;
        }
        Operator::TableCopy {
            dst_table: dst_table_index,
            src_table: src_table_index,
        } => {
            let len = state.pop1();
            let src = state.pop1();
            let dest = state.pop1();
            env.translate_table_copy(
                builder.cursor(),
                TableIndex::from_u32(*dst_table_index),
                TableIndex::from_u32(*src_table_index),
                dest,
                src,
                len,
            )?;
        }
        Operator::ElemDrop { elem_index } => {
            env.translate_elem_drop(builder.cursor(), ElemIndex::from_u32(*elem_index))?;
        }

        // threads
        // https://github.com/WebAssembly/threads
        Operator::MemoryAtomicWait32 { memarg } | Operator::MemoryAtomicWait64 { memarg } => {
            // The WebAssembly MVP only supports one linear memory and
            // wasmparser will ensure that the memory indices specified are
            // zero.
            let implied_ty = match op {
                Operator::MemoryAtomicWait64 { .. } => I64,
                Operator::MemoryAtomicWait32 { .. } => I32,
                _ => unreachable!(),
            };
            let mem_index = MemoryIndex::from_u32(memarg.memory);
            let timeout = state.pop1(); // 64 (fixed)
            let expected = state.pop1(); // 32 or 64 (per the `Ixx` in `IxxAtomicWait`)
            assert_eq!(builder.func.dfg.value_type(expected), implied_ty);
            let addr = state.pop1();
            let effective_addr = if memarg.offset == 0 {
                addr
            } else {
                // TODO let index_type = environ.mems()[mem].index_type;
                let index_type = I32;
                let offset = builder
                    .ins()
                    .iconst(index_type, i64::try_from(memarg.offset).unwrap());
                builder
                    .ins()
                    .uadd_overflow_trap(addr, offset, TrapCode::HEAP_OUT_OF_BOUNDS)
            };
            // `fn translate_atomic_wait` can inspect the type of `expected` to figure out what
            // code it needs to generate, if it wants.
            let res = env.translate_atomic_wait(
                builder.cursor(),
                mem_index,
                effective_addr,
                expected,
                timeout,
            )?;
            state.push1(res);
        }
        Operator::MemoryAtomicNotify { memarg } => {
            let mem_index = MemoryIndex::from_u32(memarg.memory);
            let count = state.pop1(); // 32 (fixed)
            let addr = state.pop1();
            let effective_addr = if memarg.offset == 0 {
                addr
            } else {
                // TODO let index_type = environ.mems()[mem].index_type;
                let index_type = I32;
                let offset = builder
                    .ins()
                    .iconst(index_type, i64::try_from(memarg.offset).unwrap());
                builder
                    .ins()
                    .uadd_overflow_trap(addr, offset, TrapCode::HEAP_OUT_OF_BOUNDS)
            };
            let res =
                env.translate_atomic_notify(builder.cursor(), mem_index, effective_addr, count)?;
            state.push1(res);
        }
        Operator::I32AtomicLoad { memarg } => {
            translate_atomic_load(I32, I32, memarg, builder, state, env)?;
        }
        Operator::I64AtomicLoad { memarg } => {
            translate_atomic_load(I64, I64, memarg, builder, state, env)?;
        }
        Operator::I32AtomicLoad8U { memarg } => {
            translate_atomic_load(I32, I8, memarg, builder, state, env)?;
        }
        Operator::I32AtomicLoad16U { memarg } => {
            translate_atomic_load(I32, I16, memarg, builder, state, env)?;
        }
        Operator::I64AtomicLoad8U { memarg } => {
            translate_atomic_load(I64, I8, memarg, builder, state, env)?;
        }
        Operator::I64AtomicLoad16U { memarg } => {
            translate_atomic_load(I64, I16, memarg, builder, state, env)?;
        }
        Operator::I64AtomicLoad32U { memarg } => {
            translate_atomic_load(I64, I32, memarg, builder, state, env)?;
        }

        Operator::I32AtomicStore { memarg } => {
            translate_atomic_store(I32, memarg, builder, state, env)?;
        }
        Operator::I64AtomicStore { memarg } => {
            translate_atomic_store(I64, memarg, builder, state, env)?;
        }
        Operator::I32AtomicStore8 { memarg } => {
            translate_atomic_store(I8, memarg, builder, state, env)?;
        }
        Operator::I32AtomicStore16 { memarg } => {
            translate_atomic_store(I16, memarg, builder, state, env)?;
        }
        Operator::I64AtomicStore8 { memarg } => {
            translate_atomic_store(I8, memarg, builder, state, env)?;
        }
        Operator::I64AtomicStore16 { memarg } => {
            translate_atomic_store(I16, memarg, builder, state, env)?;
        }
        Operator::I64AtomicStore32 { memarg } => {
            translate_atomic_store(I32, memarg, builder, state, env)?;
        }

        Operator::I32AtomicRmwAdd { memarg } => {
            translate_atomic_rmw(I32, I32, AtomicRmwOp::Add, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmwAdd { memarg } => {
            translate_atomic_rmw(I64, I64, AtomicRmwOp::Add, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw8AddU { memarg } => {
            translate_atomic_rmw(I32, I8, AtomicRmwOp::Add, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw16AddU { memarg } => {
            translate_atomic_rmw(I32, I16, AtomicRmwOp::Add, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw8AddU { memarg } => {
            translate_atomic_rmw(I64, I8, AtomicRmwOp::Add, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw16AddU { memarg } => {
            translate_atomic_rmw(I64, I16, AtomicRmwOp::Add, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw32AddU { memarg } => {
            translate_atomic_rmw(I64, I32, AtomicRmwOp::Add, memarg, builder, state, env)?;
        }

        Operator::I32AtomicRmwSub { memarg } => {
            translate_atomic_rmw(I32, I32, AtomicRmwOp::Sub, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmwSub { memarg } => {
            translate_atomic_rmw(I64, I64, AtomicRmwOp::Sub, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw8SubU { memarg } => {
            translate_atomic_rmw(I32, I8, AtomicRmwOp::Sub, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw16SubU { memarg } => {
            translate_atomic_rmw(I32, I16, AtomicRmwOp::Sub, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw8SubU { memarg } => {
            translate_atomic_rmw(I64, I8, AtomicRmwOp::Sub, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw16SubU { memarg } => {
            translate_atomic_rmw(I64, I16, AtomicRmwOp::Sub, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw32SubU { memarg } => {
            translate_atomic_rmw(I64, I32, AtomicRmwOp::Sub, memarg, builder, state, env)?;
        }

        Operator::I32AtomicRmwAnd { memarg } => {
            translate_atomic_rmw(I32, I32, AtomicRmwOp::And, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmwAnd { memarg } => {
            translate_atomic_rmw(I64, I64, AtomicRmwOp::And, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw8AndU { memarg } => {
            translate_atomic_rmw(I32, I8, AtomicRmwOp::And, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw16AndU { memarg } => {
            translate_atomic_rmw(I32, I16, AtomicRmwOp::And, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw8AndU { memarg } => {
            translate_atomic_rmw(I64, I8, AtomicRmwOp::And, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw16AndU { memarg } => {
            translate_atomic_rmw(I64, I16, AtomicRmwOp::And, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw32AndU { memarg } => {
            translate_atomic_rmw(I64, I32, AtomicRmwOp::And, memarg, builder, state, env)?;
        }

        Operator::I32AtomicRmwOr { memarg } => {
            translate_atomic_rmw(I32, I32, AtomicRmwOp::Or, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmwOr { memarg } => {
            translate_atomic_rmw(I64, I64, AtomicRmwOp::Or, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw8OrU { memarg } => {
            translate_atomic_rmw(I32, I8, AtomicRmwOp::Or, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw16OrU { memarg } => {
            translate_atomic_rmw(I32, I16, AtomicRmwOp::Or, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw8OrU { memarg } => {
            translate_atomic_rmw(I64, I8, AtomicRmwOp::Or, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw16OrU { memarg } => {
            translate_atomic_rmw(I64, I16, AtomicRmwOp::Or, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw32OrU { memarg } => {
            translate_atomic_rmw(I64, I32, AtomicRmwOp::Or, memarg, builder, state, env)?;
        }

        Operator::I32AtomicRmwXor { memarg } => {
            translate_atomic_rmw(I32, I32, AtomicRmwOp::Xor, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmwXor { memarg } => {
            translate_atomic_rmw(I64, I64, AtomicRmwOp::Xor, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw8XorU { memarg } => {
            translate_atomic_rmw(I32, I8, AtomicRmwOp::Xor, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw16XorU { memarg } => {
            translate_atomic_rmw(I32, I16, AtomicRmwOp::Xor, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw8XorU { memarg } => {
            translate_atomic_rmw(I64, I8, AtomicRmwOp::Xor, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw16XorU { memarg } => {
            translate_atomic_rmw(I64, I16, AtomicRmwOp::Xor, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw32XorU { memarg } => {
            translate_atomic_rmw(I64, I32, AtomicRmwOp::Xor, memarg, builder, state, env)?;
        }

        Operator::I32AtomicRmwXchg { memarg } => {
            translate_atomic_rmw(I32, I32, AtomicRmwOp::Xchg, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmwXchg { memarg } => {
            translate_atomic_rmw(I64, I64, AtomicRmwOp::Xchg, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw8XchgU { memarg } => {
            translate_atomic_rmw(I32, I8, AtomicRmwOp::Xchg, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw16XchgU { memarg } => {
            translate_atomic_rmw(I32, I16, AtomicRmwOp::Xchg, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw8XchgU { memarg } => {
            translate_atomic_rmw(I64, I8, AtomicRmwOp::Xchg, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw16XchgU { memarg } => {
            translate_atomic_rmw(I64, I16, AtomicRmwOp::Xchg, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw32XchgU { memarg } => {
            translate_atomic_rmw(I64, I32, AtomicRmwOp::Xchg, memarg, builder, state, env)?;
        }

        Operator::I32AtomicRmwCmpxchg { memarg } => {
            translate_atomic_cas(I32, I32, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmwCmpxchg { memarg } => {
            translate_atomic_cas(I64, I64, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw8CmpxchgU { memarg } => {
            translate_atomic_cas(I32, I8, memarg, builder, state, env)?;
        }
        Operator::I32AtomicRmw16CmpxchgU { memarg } => {
            translate_atomic_cas(I32, I16, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw8CmpxchgU { memarg } => {
            translate_atomic_cas(I64, I8, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw16CmpxchgU { memarg } => {
            translate_atomic_cas(I64, I16, memarg, builder, state, env)?;
        }
        Operator::I64AtomicRmw32CmpxchgU { memarg } => {
            translate_atomic_cas(I64, I32, memarg, builder, state, env)?;
        }
        Operator::AtomicFence { .. } => {
            builder.ins().fence();
        }

        // 128-bit SIMD
        // - https://github.com/webassembly/simd
        // - https://webassembly.github.io/simd/core/binary/instructions.html
        Operator::V128Const { value } => {
            let data = value.bytes().to_vec().into();
            let handle = builder.func.dfg.constants.insert(data);
            let value = builder.ins().vconst(I8X16, handle);
            // the v128.const is typed in CLIF as a I8x16 but bitcast to a different type
            // before use
            state.push1(value);
        }
        Operator::V128Load { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(memarg, ir::Opcode::Load, I8X16, builder, state, env)
            );
        }
        Operator::V128Load8x8S { memarg } => {
            let index = state.pop1();
            let mem = state.get_memory(builder.func, MemoryIndex::from_u32(memarg.memory), env);
            let (flags, _, base) = unwrap_or_return_unreachable_state!(
                state,
                mem.prepare_addr(builder, index, 8, memarg, env)
            );
            let loaded = builder.ins().sload8x8(flags, base, 0i32);
            state.push1(loaded);
        }
        Operator::V128Load8x8U { memarg } => {
            let index = state.pop1();
            let mem = state.get_memory(builder.func, MemoryIndex::from_u32(memarg.memory), env);
            let (flags, _, base) = unwrap_or_return_unreachable_state!(
                state,
                mem.prepare_addr(builder, index, 8, memarg, env)
            );
            let loaded = builder.ins().uload8x8(flags, base, 0i32);
            state.push1(loaded);
        }
        Operator::V128Load16x4S { memarg } => {
            let index = state.pop1();
            let mem = state.get_memory(builder.func, MemoryIndex::from_u32(memarg.memory), env);
            let (flags, _, base) = unwrap_or_return_unreachable_state!(
                state,
                mem.prepare_addr(builder, index, 8, memarg, env)
            );
            let loaded = builder.ins().sload16x4(flags, base, 0i32);
            state.push1(loaded);
        }
        Operator::V128Load16x4U { memarg } => {
            let index = state.pop1();
            let mem = state.get_memory(builder.func, MemoryIndex::from_u32(memarg.memory), env);
            let (flags, _, base) = unwrap_or_return_unreachable_state!(
                state,
                mem.prepare_addr(builder, index, 8, memarg, env)
            );
            let loaded = builder.ins().uload16x4(flags, base, 0i32);
            state.push1(loaded);
        }
        Operator::V128Load32x2S { memarg } => {
            let index = state.pop1();
            let mem = state.get_memory(builder.func, MemoryIndex::from_u32(memarg.memory), env);
            let (flags, _, base) = unwrap_or_return_unreachable_state!(
                state,
                mem.prepare_addr(builder, index, 8, memarg, env)
            );
            let loaded = builder.ins().sload32x2(flags, base, 0i32);
            state.push1(loaded);
        }
        Operator::V128Load32x2U { memarg } => {
            let index = state.pop1();
            let mem = state.get_memory(builder.func, MemoryIndex::from_u32(memarg.memory), env);
            let (flags, _, base) = unwrap_or_return_unreachable_state!(
                state,
                mem.prepare_addr(builder, index, 8, memarg, env)
            );
            let loaded = builder.ins().uload32x2(flags, base, 0i32);
            state.push1(loaded);
        }
        Operator::V128Store { .. } => todo!(),
        Operator::I8x16Splat | Operator::I16x8Splat => {
            let reduced = builder.ins().ireduce(type_of(op).lane_type(), state.pop1());
            let splatted = builder.ins().splat(type_of(op), reduced);
            state.push1(splatted);
        }
        Operator::I32x4Splat
        | Operator::I64x2Splat
        | Operator::F32x4Splat
        | Operator::F64x2Splat => {
            let splatted = builder.ins().splat(type_of(op), state.pop1());
            state.push1(splatted);
        }
        Operator::V128Load8Splat { memarg }
        | Operator::V128Load16Splat { memarg }
        | Operator::V128Load32Splat { memarg }
        | Operator::V128Load64Splat { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(
                    memarg,
                    ir::Opcode::Load,
                    type_of(op).lane_type(),
                    builder,
                    state,
                    env,
                )
            );
            let splatted = builder.ins().splat(type_of(op), state.pop1());
            state.push1(splatted);
        }
        Operator::V128Load32Zero { memarg } | Operator::V128Load64Zero { memarg } => {
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(
                    memarg,
                    ir::Opcode::Load,
                    type_of(op).lane_type(),
                    builder,
                    state,
                    env,
                )
            );
            let as_vector = builder.ins().scalar_to_vector(type_of(op), state.pop1());
            state.push1(as_vector);
        }
        Operator::V128Load8Lane { memarg, lane }
        | Operator::V128Load16Lane { memarg, lane }
        | Operator::V128Load32Lane { memarg, lane }
        | Operator::V128Load64Lane { memarg, lane } => {
            let vector = pop1_with_bitcast(state, type_of(op), builder);
            unwrap_or_return_unreachable_state!(
                state,
                translate_load(
                    memarg,
                    ir::Opcode::Load,
                    type_of(op).lane_type(),
                    builder,
                    state,
                    env,
                )
            );
            let replacement = state.pop1();
            state.push1(builder.ins().insertlane(vector, replacement, *lane));
        }
        Operator::V128Store8Lane { memarg, lane }
        | Operator::V128Store16Lane { memarg, lane }
        | Operator::V128Store32Lane { memarg, lane }
        | Operator::V128Store64Lane { memarg, lane } => {
            let vector = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().extractlane(vector, *lane));
            translate_store(memarg, ir::Opcode::Store, builder, state, env)?;
        }
        Operator::I8x16ExtractLaneS { lane } | Operator::I16x8ExtractLaneS { lane } => {
            let vector = pop1_with_bitcast(state, type_of(op), builder);
            let extracted = builder.ins().extractlane(vector, *lane);
            state.push1(builder.ins().sextend(I32, extracted));
        }
        Operator::I8x16ExtractLaneU { lane } | Operator::I16x8ExtractLaneU { lane } => {
            let vector = pop1_with_bitcast(state, type_of(op), builder);
            let extracted = builder.ins().extractlane(vector, *lane);
            state.push1(builder.ins().uextend(I32, extracted));
            // On x86, PEXTRB zeroes the upper bits of the destination register of extractlane so
            // uextend could be elided; for now, uextend is needed for Cranelift's type checks to
            // work.
        }
        Operator::I32x4ExtractLane { lane }
        | Operator::I64x2ExtractLane { lane }
        | Operator::F32x4ExtractLane { lane }
        | Operator::F64x2ExtractLane { lane } => {
            let vector = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().extractlane(vector, *lane));
        }
        Operator::I8x16ReplaceLane { lane } | Operator::I16x8ReplaceLane { lane } => {
            let (vector, replacement) = state.pop2();
            let ty = type_of(op);
            let reduced = builder.ins().ireduce(ty.lane_type(), replacement);
            let vector = optionally_bitcast_vector(vector, ty, builder);
            state.push1(builder.ins().insertlane(vector, reduced, *lane));
        }
        Operator::I32x4ReplaceLane { lane }
        | Operator::I64x2ReplaceLane { lane }
        | Operator::F32x4ReplaceLane { lane }
        | Operator::F64x2ReplaceLane { lane } => {
            let (vector, replacement) = state.pop2();
            let vector = optionally_bitcast_vector(vector, type_of(op), builder);
            state.push1(builder.ins().insertlane(vector, replacement, *lane));
        }
        Operator::I8x16Shuffle { lanes, .. } => {
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            let lanes = ConstantData::from(lanes.as_ref());
            let mask = builder.func.dfg.immediates.push(lanes);
            let shuffled = builder.ins().shuffle(a, b, mask);
            state.push1(shuffled);
            // At this point the original types of a and b are lost; users of this value (i.e. this
            // WASM-to-CLIF translator) may need to bitcast for type-correctness. This is due
            // to WASM using the less specific v128 type for certain operations and more specific
            // types (e.g. i8x16) for others.
        }
        Operator::I8x16Swizzle => {
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            state.push1(builder.ins().swizzle(a, b));
        }
        Operator::I8x16Add | Operator::I16x8Add | Operator::I32x4Add | Operator::I64x2Add => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().iadd(a, b));
        }
        Operator::I8x16AddSatS | Operator::I16x8AddSatS => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().sadd_sat(a, b));
        }
        Operator::I8x16AddSatU | Operator::I16x8AddSatU => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().uadd_sat(a, b));
        }
        Operator::I8x16Sub | Operator::I16x8Sub | Operator::I32x4Sub | Operator::I64x2Sub => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().isub(a, b));
        }
        Operator::I8x16SubSatS | Operator::I16x8SubSatS => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().ssub_sat(a, b));
        }
        Operator::I8x16SubSatU | Operator::I16x8SubSatU => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().usub_sat(a, b));
        }
        Operator::I8x16MinS | Operator::I16x8MinS | Operator::I32x4MinS => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().smin(a, b));
        }
        Operator::I8x16MinU | Operator::I16x8MinU | Operator::I32x4MinU => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().umin(a, b));
        }
        Operator::I8x16MaxS | Operator::I16x8MaxS | Operator::I32x4MaxS => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().smax(a, b));
        }
        Operator::I8x16MaxU | Operator::I16x8MaxU | Operator::I32x4MaxU => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().umax(a, b));
        }
        Operator::I8x16AvgrU | Operator::I16x8AvgrU => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().avg_round(a, b));
        }
        Operator::I8x16Neg | Operator::I16x8Neg | Operator::I32x4Neg | Operator::I64x2Neg => {
            let a = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().ineg(a));
        }
        Operator::I8x16Abs | Operator::I16x8Abs | Operator::I32x4Abs | Operator::I64x2Abs => {
            let a = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().iabs(a));
        }
        Operator::I16x8Mul | Operator::I32x4Mul | Operator::I64x2Mul => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().imul(a, b));
        }
        Operator::V128Or => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().bor(a, b));
        }
        Operator::V128Xor => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().bxor(a, b));
        }
        Operator::V128And => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().band(a, b));
        }
        Operator::V128AndNot => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().band_not(a, b));
        }
        Operator::V128Not => {
            let a = state.pop1();
            state.push1(builder.ins().bnot(a));
        }
        Operator::I8x16Shl | Operator::I16x8Shl | Operator::I32x4Shl | Operator::I64x2Shl => {
            let (a, b) = state.pop2();
            let bitcast_a = optionally_bitcast_vector(a, type_of(op), builder);
            // The spec expects to shift with `b mod lanewidth`; This is directly compatible
            // with cranelift's instruction.
            state.push1(builder.ins().ishl(bitcast_a, b));
        }
        Operator::I8x16ShrU | Operator::I16x8ShrU | Operator::I32x4ShrU | Operator::I64x2ShrU => {
            let (a, b) = state.pop2();
            let bitcast_a = optionally_bitcast_vector(a, type_of(op), builder);
            // The spec expects to shift with `b mod lanewidth`; This is directly compatible
            // with cranelift's instruction.
            state.push1(builder.ins().ushr(bitcast_a, b));
        }
        Operator::I8x16ShrS | Operator::I16x8ShrS | Operator::I32x4ShrS | Operator::I64x2ShrS => {
            let (a, b) = state.pop2();
            let bitcast_a = optionally_bitcast_vector(a, type_of(op), builder);
            // The spec expects to shift with `b mod lanewidth`; This is directly compatible
            // with cranelift's instruction.
            state.push1(builder.ins().sshr(bitcast_a, b));
        }
        Operator::V128Bitselect => {
            let (a, b, c) = pop3_with_bitcast(state, I8X16, builder);
            // The CLIF operand ordering is slightly different and the types of all three
            // operands must match (hence the bitcast).
            state.push1(builder.ins().bitselect(c, a, b));
        }
        Operator::V128AnyTrue => {
            let a = pop1_with_bitcast(state, type_of(op), builder);
            let bool_result = builder.ins().vany_true(a);
            state.push1(builder.ins().uextend(I32, bool_result));
        }
        Operator::I8x16AllTrue
        | Operator::I16x8AllTrue
        | Operator::I32x4AllTrue
        | Operator::I64x2AllTrue => {
            let a = pop1_with_bitcast(state, type_of(op), builder);
            let bool_result = builder.ins().vall_true(a);
            state.push1(builder.ins().uextend(I32, bool_result));
        }
        Operator::I8x16Bitmask
        | Operator::I16x8Bitmask
        | Operator::I32x4Bitmask
        | Operator::I64x2Bitmask => {
            let a = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().vhigh_bits(I32, a));
        }
        Operator::I8x16Eq | Operator::I16x8Eq | Operator::I32x4Eq | Operator::I64x2Eq => {
            translate_vector_icmp(IntCC::Equal, type_of(op), builder, state);
        }
        Operator::I8x16Ne | Operator::I16x8Ne | Operator::I32x4Ne | Operator::I64x2Ne => {
            translate_vector_icmp(IntCC::NotEqual, type_of(op), builder, state);
        }
        Operator::I8x16GtS | Operator::I16x8GtS | Operator::I32x4GtS | Operator::I64x2GtS => {
            translate_vector_icmp(IntCC::SignedGreaterThan, type_of(op), builder, state);
        }
        Operator::I8x16LtS | Operator::I16x8LtS | Operator::I32x4LtS | Operator::I64x2LtS => {
            translate_vector_icmp(IntCC::SignedLessThan, type_of(op), builder, state);
        }
        Operator::I8x16GtU | Operator::I16x8GtU | Operator::I32x4GtU => {
            translate_vector_icmp(IntCC::UnsignedGreaterThan, type_of(op), builder, state);
        }
        Operator::I8x16LtU | Operator::I16x8LtU | Operator::I32x4LtU => {
            translate_vector_icmp(IntCC::UnsignedLessThan, type_of(op), builder, state);
        }
        Operator::I8x16GeS | Operator::I16x8GeS | Operator::I32x4GeS | Operator::I64x2GeS => {
            translate_vector_icmp(IntCC::SignedGreaterThanOrEqual, type_of(op), builder, state);
        }
        Operator::I8x16LeS | Operator::I16x8LeS | Operator::I32x4LeS | Operator::I64x2LeS => {
            translate_vector_icmp(IntCC::SignedLessThanOrEqual, type_of(op), builder, state);
        }
        Operator::I8x16GeU | Operator::I16x8GeU | Operator::I32x4GeU => translate_vector_icmp(
            IntCC::UnsignedGreaterThanOrEqual,
            type_of(op),
            builder,
            state,
        ),
        Operator::I8x16LeU | Operator::I16x8LeU | Operator::I32x4LeU => {
            translate_vector_icmp(IntCC::UnsignedLessThanOrEqual, type_of(op), builder, state);
        }
        Operator::F32x4Eq | Operator::F64x2Eq => {
            translate_vector_fcmp(FloatCC::Equal, type_of(op), builder, state);
        }
        Operator::F32x4Ne | Operator::F64x2Ne => {
            translate_vector_fcmp(FloatCC::NotEqual, type_of(op), builder, state);
        }
        Operator::F32x4Lt | Operator::F64x2Lt => {
            translate_vector_fcmp(FloatCC::LessThan, type_of(op), builder, state);
        }
        Operator::F32x4Gt | Operator::F64x2Gt => {
            translate_vector_fcmp(FloatCC::GreaterThan, type_of(op), builder, state);
        }
        Operator::F32x4Le | Operator::F64x2Le => {
            translate_vector_fcmp(FloatCC::LessThanOrEqual, type_of(op), builder, state);
        }
        Operator::F32x4Ge | Operator::F64x2Ge => {
            translate_vector_fcmp(FloatCC::GreaterThanOrEqual, type_of(op), builder, state);
        }
        Operator::F32x4Add | Operator::F64x2Add => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().fadd(a, b));
        }
        Operator::F32x4Sub | Operator::F64x2Sub => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().fsub(a, b));
        }
        Operator::F32x4Mul | Operator::F64x2Mul => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().fmul(a, b));
        }
        Operator::F32x4Div | Operator::F64x2Div => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().fdiv(a, b));
        }
        Operator::F32x4Max | Operator::F64x2Max => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().fmax(a, b));
        }
        Operator::F32x4Min | Operator::F64x2Min => {
            let (a, b) = pop2_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().fmin(a, b));
        }
        Operator::F32x4PMax | Operator::F64x2PMax => {
            // Note the careful ordering here with respect to `fcmp` and
            // `bitselect`. This matches the spec definition of:
            //
            //  fpmax(z1, z2) =
            //      * If z1 is less than z2 then return z2.
            //      * Else return z1.
            let ty = type_of(op);
            let (a, b) = pop2_with_bitcast(state, ty, builder);
            let cmp = builder.ins().fcmp(FloatCC::LessThan, a, b);
            let cmp = optionally_bitcast_vector(cmp, ty, builder);
            state.push1(builder.ins().bitselect(cmp, b, a));
        }
        Operator::F32x4PMin | Operator::F64x2PMin => {
            // Note the careful ordering here which is similar to `pmax` above:
            //
            //  fpmin(z1, z2) =
            //      * If z2 is less than z1 then return z2.
            //      * Else return z1.
            let ty = type_of(op);
            let (a, b) = pop2_with_bitcast(state, ty, builder);
            let cmp = builder.ins().fcmp(FloatCC::LessThan, b, a);
            let cmp = optionally_bitcast_vector(cmp, ty, builder);
            state.push1(builder.ins().bitselect(cmp, b, a));
        }
        Operator::F32x4Sqrt | Operator::F64x2Sqrt => {
            let a = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().sqrt(a));
        }
        Operator::F32x4Neg | Operator::F64x2Neg => {
            let a = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().fneg(a));
        }
        Operator::F32x4Abs | Operator::F64x2Abs => {
            let a = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().fabs(a));
        }
        Operator::F32x4ConvertI32x4S => {
            let a = pop1_with_bitcast(state, I32X4, builder);
            state.push1(builder.ins().fcvt_from_sint(F32X4, a));
        }
        Operator::F32x4ConvertI32x4U => {
            let a = pop1_with_bitcast(state, I32X4, builder);
            state.push1(builder.ins().fcvt_from_uint(F32X4, a));
        }
        Operator::F64x2ConvertLowI32x4S => {
            let a = pop1_with_bitcast(state, I32X4, builder);
            let widened_a = builder.ins().swiden_low(a);
            state.push1(builder.ins().fcvt_from_sint(F64X2, widened_a));
        }
        Operator::F64x2ConvertLowI32x4U => {
            let a = pop1_with_bitcast(state, I32X4, builder);
            let widened_a = builder.ins().uwiden_low(a);
            state.push1(builder.ins().fcvt_from_uint(F64X2, widened_a));
        }
        Operator::F64x2PromoteLowF32x4 => {
            let a = pop1_with_bitcast(state, F32X4, builder);
            state.push1(builder.ins().fvpromote_low(a));
        }
        Operator::F32x4DemoteF64x2Zero => {
            let a = pop1_with_bitcast(state, F64X2, builder);
            state.push1(builder.ins().fvdemote(a));
        }
        Operator::I32x4TruncSatF32x4S => {
            let a = pop1_with_bitcast(state, F32X4, builder);
            state.push1(builder.ins().fcvt_to_sint_sat(I32X4, a));
        }
        Operator::I32x4TruncSatF64x2SZero => {
            let a = pop1_with_bitcast(state, F64X2, builder);
            let converted_a = builder.ins().fcvt_to_sint_sat(I64X2, a);
            let handle = builder.func.dfg.constants.insert(vec![0u8; 16].into());
            let zero = builder.ins().vconst(I64X2, handle);

            state.push1(builder.ins().snarrow(converted_a, zero));
        }
        // FIXME(#5913): the relaxed instructions here are translated the same
        // as the saturating instructions, even when the code generator
        // configuration allow for different semantics across hosts. On x86,
        // however, it's theoretically possible to have a slightly more optimal
        // lowering which accounts for NaN differently, although the lowering is
        // still not trivial (e.g. one instruction). At this time the
        // more-optimal-but-still-large lowering for x86 is not implemented so
        // the relaxed instructions are listed here instead of down below with
        // the other relaxed instructions. An x86-specific implementation (or
        // perhaps for other backends too) should be added and the codegen for
        // the relaxed instruction should conditionally be different.
        Operator::I32x4RelaxedTruncF32x4U | Operator::I32x4TruncSatF32x4U => {
            let a = pop1_with_bitcast(state, F32X4, builder);
            state.push1(builder.ins().fcvt_to_uint_sat(I32X4, a));
        }
        Operator::I32x4RelaxedTruncF64x2UZero | Operator::I32x4TruncSatF64x2UZero => {
            let a = pop1_with_bitcast(state, F64X2, builder);
            let converted_a = builder.ins().fcvt_to_uint_sat(I64X2, a);
            let handle = builder.func.dfg.constants.insert(vec![0u8; 16].into());
            let zero = builder.ins().vconst(I64X2, handle);

            state.push1(builder.ins().uunarrow(converted_a, zero));
        }
        Operator::I8x16NarrowI16x8S => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            state.push1(builder.ins().snarrow(a, b));
        }
        Operator::I16x8NarrowI32x4S => {
            let (a, b) = pop2_with_bitcast(state, I32X4, builder);
            state.push1(builder.ins().snarrow(a, b));
        }
        Operator::I8x16NarrowI16x8U => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            state.push1(builder.ins().unarrow(a, b));
        }
        Operator::I16x8NarrowI32x4U => {
            let (a, b) = pop2_with_bitcast(state, I32X4, builder);
            state.push1(builder.ins().unarrow(a, b));
        }
        Operator::I16x8ExtendLowI8x16S => {
            let a = pop1_with_bitcast(state, I8X16, builder);
            state.push1(builder.ins().swiden_low(a));
        }
        Operator::I16x8ExtendHighI8x16S => {
            let a = pop1_with_bitcast(state, I8X16, builder);
            state.push1(builder.ins().swiden_high(a));
        }
        Operator::I16x8ExtendLowI8x16U => {
            let a = pop1_with_bitcast(state, I8X16, builder);
            state.push1(builder.ins().uwiden_low(a));
        }
        Operator::I16x8ExtendHighI8x16U => {
            let a = pop1_with_bitcast(state, I8X16, builder);
            state.push1(builder.ins().uwiden_high(a));
        }
        Operator::I32x4ExtendLowI16x8S => {
            let a = pop1_with_bitcast(state, I16X8, builder);
            state.push1(builder.ins().swiden_low(a));
        }
        Operator::I32x4ExtendHighI16x8S => {
            let a = pop1_with_bitcast(state, I16X8, builder);
            state.push1(builder.ins().swiden_high(a));
        }
        Operator::I32x4ExtendLowI16x8U => {
            let a = pop1_with_bitcast(state, I16X8, builder);
            state.push1(builder.ins().uwiden_low(a));
        }
        Operator::I32x4ExtendHighI16x8U => {
            let a = pop1_with_bitcast(state, I16X8, builder);
            state.push1(builder.ins().uwiden_high(a));
        }
        Operator::I64x2ExtendLowI32x4S => {
            let a = pop1_with_bitcast(state, I32X4, builder);
            state.push1(builder.ins().swiden_low(a));
        }
        Operator::I64x2ExtendHighI32x4S => {
            let a = pop1_with_bitcast(state, I32X4, builder);
            state.push1(builder.ins().swiden_high(a));
        }
        Operator::I64x2ExtendLowI32x4U => {
            let a = pop1_with_bitcast(state, I32X4, builder);
            state.push1(builder.ins().uwiden_low(a));
        }
        Operator::I64x2ExtendHighI32x4U => {
            let a = pop1_with_bitcast(state, I32X4, builder);
            state.push1(builder.ins().uwiden_high(a));
        }
        Operator::I16x8ExtAddPairwiseI8x16S => {
            let a = pop1_with_bitcast(state, I8X16, builder);
            let widen_low = builder.ins().swiden_low(a);
            let widen_high = builder.ins().swiden_high(a);
            state.push1(builder.ins().iadd_pairwise(widen_low, widen_high));
        }
        Operator::I32x4ExtAddPairwiseI16x8S => {
            let a = pop1_with_bitcast(state, I16X8, builder);
            let widen_low = builder.ins().swiden_low(a);
            let widen_high = builder.ins().swiden_high(a);
            state.push1(builder.ins().iadd_pairwise(widen_low, widen_high));
        }
        Operator::I16x8ExtAddPairwiseI8x16U => {
            let a = pop1_with_bitcast(state, I8X16, builder);
            let widen_low = builder.ins().uwiden_low(a);
            let widen_high = builder.ins().uwiden_high(a);
            state.push1(builder.ins().iadd_pairwise(widen_low, widen_high));
        }
        Operator::I32x4ExtAddPairwiseI16x8U => {
            let a = pop1_with_bitcast(state, I16X8, builder);
            let widen_low = builder.ins().uwiden_low(a);
            let widen_high = builder.ins().uwiden_high(a);
            state.push1(builder.ins().iadd_pairwise(widen_low, widen_high));
        }
        Operator::F32x4Ceil | Operator::F64x2Ceil => {
            // This is something of a misuse of `type_of`, because that produces the return type
            // of `op`.  In this case we want the arg type, but we know it's the same as the
            // return type.  Same for the 3 cases below.
            let arg = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().ceil(arg));
        }
        Operator::F32x4Floor | Operator::F64x2Floor => {
            let arg = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().floor(arg));
        }
        Operator::F32x4Trunc | Operator::F64x2Trunc => {
            let arg = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().trunc(arg));
        }
        Operator::F32x4Nearest | Operator::F64x2Nearest => {
            let arg = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().nearest(arg));
        }
        Operator::I32x4DotI16x8S => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            let alo = builder.ins().swiden_low(a);
            let blo = builder.ins().swiden_low(b);
            let lo = builder.ins().imul(alo, blo);
            let ahi = builder.ins().swiden_high(a);
            let bhi = builder.ins().swiden_high(b);
            let hi = builder.ins().imul(ahi, bhi);
            state.push1(builder.ins().iadd_pairwise(lo, hi));
        }
        Operator::I8x16Popcnt => {
            let arg = pop1_with_bitcast(state, type_of(op), builder);
            state.push1(builder.ins().popcnt(arg));
        }
        Operator::I16x8Q15MulrSatS => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            state.push1(builder.ins().sqmul_round_sat(a, b));
        }
        Operator::I16x8ExtMulLowI8x16S => {
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            let a_low = builder.ins().swiden_low(a);
            let b_low = builder.ins().swiden_low(b);
            state.push1(builder.ins().imul(a_low, b_low));
        }
        Operator::I16x8ExtMulHighI8x16S => {
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            let a_high = builder.ins().swiden_high(a);
            let b_high = builder.ins().swiden_high(b);
            state.push1(builder.ins().imul(a_high, b_high));
        }
        Operator::I16x8ExtMulLowI8x16U => {
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            let a_low = builder.ins().uwiden_low(a);
            let b_low = builder.ins().uwiden_low(b);
            state.push1(builder.ins().imul(a_low, b_low));
        }
        Operator::I16x8ExtMulHighI8x16U => {
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            let a_high = builder.ins().uwiden_high(a);
            let b_high = builder.ins().uwiden_high(b);
            state.push1(builder.ins().imul(a_high, b_high));
        }
        Operator::I32x4ExtMulLowI16x8S => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            let a_low = builder.ins().swiden_low(a);
            let b_low = builder.ins().swiden_low(b);
            state.push1(builder.ins().imul(a_low, b_low));
        }
        Operator::I32x4ExtMulHighI16x8S => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            let a_high = builder.ins().swiden_high(a);
            let b_high = builder.ins().swiden_high(b);
            state.push1(builder.ins().imul(a_high, b_high));
        }
        Operator::I32x4ExtMulLowI16x8U => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            let a_low = builder.ins().uwiden_low(a);
            let b_low = builder.ins().uwiden_low(b);
            state.push1(builder.ins().imul(a_low, b_low));
        }
        Operator::I32x4ExtMulHighI16x8U => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            let a_high = builder.ins().uwiden_high(a);
            let b_high = builder.ins().uwiden_high(b);
            state.push1(builder.ins().imul(a_high, b_high));
        }
        Operator::I64x2ExtMulLowI32x4S => {
            let (a, b) = pop2_with_bitcast(state, I32X4, builder);
            let a_low = builder.ins().swiden_low(a);
            let b_low = builder.ins().swiden_low(b);
            state.push1(builder.ins().imul(a_low, b_low));
        }
        Operator::I64x2ExtMulHighI32x4S => {
            let (a, b) = pop2_with_bitcast(state, I32X4, builder);
            let a_high = builder.ins().swiden_high(a);
            let b_high = builder.ins().swiden_high(b);
            state.push1(builder.ins().imul(a_high, b_high));
        }
        Operator::I64x2ExtMulLowI32x4U => {
            let (a, b) = pop2_with_bitcast(state, I32X4, builder);
            let a_low = builder.ins().uwiden_low(a);
            let b_low = builder.ins().uwiden_low(b);
            state.push1(builder.ins().imul(a_low, b_low));
        }
        Operator::I64x2ExtMulHighI32x4U => {
            let (a, b) = pop2_with_bitcast(state, I32X4, builder);
            let a_high = builder.ins().uwiden_high(a);
            let b_high = builder.ins().uwiden_high(b);
            state.push1(builder.ins().imul(a_high, b_high));
        }

        // Relaxed SIMD operators
        // https://github.com/WebAssembly/relaxed-simd
        Operator::F32x4RelaxedMax | Operator::F64x2RelaxedMax => {
            let ty = type_of(op);
            let (a, b) = pop2_with_bitcast(state, ty, builder);
            state.push1(if env.relaxed_simd_deterministic() || !env.is_x86() {
                // Deterministic semantics match the `fmax` instruction, or
                // the `fAAxBB.max` wasm instruction.
                builder.ins().fmax(a, b)
            } else {
                // Note that this matches the `pmax` translation which has
                // careful ordering of its operands to trigger
                // pattern-matches in the x86 backend.
                let cmp = builder.ins().fcmp(FloatCC::LessThan, a, b);
                let cmp = optionally_bitcast_vector(cmp, ty, builder);
                builder.ins().bitselect(cmp, b, a)
            });
        }

        Operator::F32x4RelaxedMin | Operator::F64x2RelaxedMin => {
            let ty = type_of(op);
            let (a, b) = pop2_with_bitcast(state, ty, builder);
            state.push1(if env.relaxed_simd_deterministic() || !env.is_x86() {
                // Deterministic semantics match the `fmin` instruction, or
                // the `fAAxBB.min` wasm instruction.
                builder.ins().fmin(a, b)
            } else {
                // Note that this matches the `pmin` translation which has
                // careful ordering of its operands to trigger
                // pattern-matches in the x86 backend.
                let cmp = builder.ins().fcmp(FloatCC::LessThan, b, a);
                let cmp = optionally_bitcast_vector(cmp, ty, builder);
                builder.ins().bitselect(cmp, b, a)
            });
        }

        Operator::I8x16RelaxedSwizzle => {
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            state.push1(
                if env.relaxed_simd_deterministic() || !env.use_x86_pshufb_for_relaxed_swizzle() {
                    // Deterministic semantics match the `i8x16.swizzle`
                    // instruction which is the CLIF `swizzle`.
                    builder.ins().swizzle(a, b)
                } else {
                    builder.ins().x86_pshufb(a, b)
                },
            );
        }

        Operator::F32x4RelaxedMadd | Operator::F64x2RelaxedMadd => {
            let (a, b, c) = pop3_with_bitcast(state, type_of(op), builder);
            state.push1(
                if env.relaxed_simd_deterministic() || env.has_native_fma() {
                    // Deterministic semantics are "fused multiply and add"
                    // which the CLIF `fma` guarantees.
                    builder.ins().fma(a, b, c)
                } else {
                    let mul = builder.ins().fmul(a, b);
                    builder.ins().fadd(mul, c)
                },
            );
        }
        Operator::F32x4RelaxedNmadd | Operator::F64x2RelaxedNmadd => {
            let (a, b, c) = pop3_with_bitcast(state, type_of(op), builder);
            let a = builder.ins().fneg(a);
            state.push1(
                if env.relaxed_simd_deterministic() || env.has_native_fma() {
                    // Deterministic semantics are "fused multiply and add"
                    // which the CLIF `fma` guarantees.
                    builder.ins().fma(a, b, c)
                } else {
                    let mul = builder.ins().fmul(a, b);
                    builder.ins().fadd(mul, c)
                },
            );
        }

        Operator::I8x16RelaxedLaneselect
        | Operator::I16x8RelaxedLaneselect
        | Operator::I32x4RelaxedLaneselect
        | Operator::I64x2RelaxedLaneselect => {
            let ty = type_of(op);
            let (a, b, c) = pop3_with_bitcast(state, ty, builder);
            // Note that the variable swaps here are intentional due to
            // the difference of the order of the wasm op and the clif
            // op.
            state.push1(
                if env.relaxed_simd_deterministic()
                    || !env.use_x86_blendv_for_relaxed_laneselect(ty)
                {
                    // Deterministic semantics are a `bitselect` along the lines
                    // of the wasm `v128.bitselect` instruction.
                    builder.ins().bitselect(c, a, b)
                } else {
                    builder.ins().x86_blendv(c, a, b)
                },
            );
        }

        Operator::I32x4RelaxedTruncF32x4S => {
            let a = pop1_with_bitcast(state, F32X4, builder);
            state.push1(if env.relaxed_simd_deterministic() || !env.is_x86() {
                // Deterministic semantics are to match the
                // `i32x4.trunc_sat_f32x4_s` instruction.
                builder.ins().fcvt_to_sint_sat(I32X4, a)
            } else {
                builder.ins().x86_cvtt2dq(I32X4, a)
            });
        }
        Operator::I32x4RelaxedTruncF64x2SZero => {
            let a = pop1_with_bitcast(state, F64X2, builder);
            let converted_a = if env.relaxed_simd_deterministic() || !env.is_x86() {
                // Deterministic semantics are to match the
                // `i32x4.trunc_sat_f64x2_s_zero` instruction.
                builder.ins().fcvt_to_sint_sat(I64X2, a)
            } else {
                builder.ins().x86_cvtt2dq(I64X2, a)
            };
            let handle = builder.func.dfg.constants.insert(vec![0u8; 16].into());
            let zero = builder.ins().vconst(I64X2, handle);

            state.push1(builder.ins().snarrow(converted_a, zero));
        }
        Operator::I16x8RelaxedQ15mulrS => {
            let (a, b) = pop2_with_bitcast(state, I16X8, builder);
            state.push1(
                if env.relaxed_simd_deterministic() || !env.use_x86_pmulhrsw_for_relaxed_q15mul() {
                    // Deterministic semantics are to match the
                    // `i16x8.q15mulr_sat_s` instruction.
                    builder.ins().sqmul_round_sat(a, b)
                } else {
                    builder.ins().x86_pmulhrsw(a, b)
                },
            );
        }
        Operator::I16x8RelaxedDotI8x16I7x16S => {
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            state.push1(
                if env.relaxed_simd_deterministic() || !env.use_x86_pmaddubsw_for_dot() {
                    // Deterministic semantics are to treat both operands as
                    // signed integers and perform the dot product.
                    let alo = builder.ins().swiden_low(a);
                    let blo = builder.ins().swiden_low(b);
                    let lo = builder.ins().imul(alo, blo);
                    let ahi = builder.ins().swiden_high(a);
                    let bhi = builder.ins().swiden_high(b);
                    let hi = builder.ins().imul(ahi, bhi);
                    builder.ins().iadd_pairwise(lo, hi)
                } else {
                    builder.ins().x86_pmaddubsw(a, b)
                },
            );
        }

        Operator::I32x4RelaxedDotI8x16I7x16AddS => {
            let c = pop1_with_bitcast(state, I32X4, builder);
            let (a, b) = pop2_with_bitcast(state, I8X16, builder);
            let dot = if env.relaxed_simd_deterministic() || !env.use_x86_pmaddubsw_for_dot() {
                // Deterministic semantics are to treat both operands as
                // signed integers and perform the dot product.
                let alo = builder.ins().swiden_low(a);
                let blo = builder.ins().swiden_low(b);
                let lo = builder.ins().imul(alo, blo);
                let ahi = builder.ins().swiden_high(a);
                let bhi = builder.ins().swiden_high(b);
                let hi = builder.ins().imul(ahi, bhi);
                builder.ins().iadd_pairwise(lo, hi)
            } else {
                builder.ins().x86_pmaddubsw(a, b)
            };
            let dotlo = builder.ins().swiden_low(dot);
            let dothi = builder.ins().swiden_high(dot);
            let dot32 = builder.ins().iadd_pairwise(dotlo, dothi);
            state.push1(builder.ins().iadd(dot32, c));
        }

        // Typed Function references
        // https://github.com/WebAssembly/function-references
        Operator::CallRef { type_index } => {
            // Get function signature
            // `index` is the index of the function's signature and `table_index` is the index of
            // the table to search the function in.
            let (sigref, num_args) =
                state.get_indirect_sig(builder.func, TypeIndex::from_u32(*type_index), env);
            let callee = state.pop1();

            // Get the `callee` operand type and check whether it's `Some(Some(<reference type>))` and
            // that the reference type is non-nullable in which case we can omit the null check.
            // If `get_operand_type` returns `Some(None)` that means it doesn't know in which case we
            // default to explicit null checks too.
            let ty = validator.get_operand_type(0);
            let needs_null_check = ty.expect("expected operand on stack").is_none_or(|ty| {
                let ty = ty.as_reference_type().expect("expected reference type");

                ty.is_nullable()
            });

            // Bitcast any vector arguments to their default type, I8X16, before calling.
            let args = state.peekn_mut(num_args);
            bitcast_wasm_params(sigref, args, builder, env);

            let call = env.translate_call_ref(
                builder,
                sigref,
                callee,
                state.peekn(num_args),
                needs_null_check,
            )?;

            let inst_results = builder.inst_results(call);
            debug_assert_eq!(
                inst_results.len(),
                builder.func.dfg.signatures[sigref].returns.len(),
                "translate_call_ref results should match the call signature"
            );
            state.popn(num_args);
            state.pushn(inst_results);
        }
        Operator::RefAsNonNull => {
            let r = state.pop1();
            let is_null = env.translate_ref_is_null(builder.cursor(), r);
            builder.ins().trapnz(is_null, TRAP_NULL_REFERENCE);
            state.push1(r);
        }
        Operator::BrOnNull { relative_depth } => {
            let r = state.pop1();
            let (br_destination, inputs) = translate_br_if_args(*relative_depth, state);
            let is_null = env.translate_ref_is_null(builder.cursor(), r);
            let else_block = builder.create_block();
            canonicalise_brif(builder, is_null, br_destination, inputs, else_block, &[]);

            builder.seal_block(else_block); // The only predecessor is the current block.
            builder.switch_to_block(else_block);
            state.push1(r);
        }
        Operator::BrOnNonNull { relative_depth } => {
            // We write this a bit differently from the spec to avoid an extra
            // block/branch and the typed accounting thereof. Instead of the
            // spec's approach, it's described as such:
            // Peek the value val from the stack.
            // If val is ref.null ht, then: pop the value val from the stack.
            // Else: Execute the instruction (br relative_depth).
            let is_null = env.translate_ref_is_null(builder.cursor(), state.peek1());
            let (br_destination, inputs) = translate_br_if_args(*relative_depth, state);
            let else_block = builder.create_block();
            canonicalise_brif(builder, is_null, else_block, &[], br_destination, inputs);

            // In the null case, pop the ref
            state.pop1();

            builder.seal_block(else_block); // The only predecessor is the current block.

            // The rest of the translation operates on our is null case, which is
            // currently an empty block
            builder.switch_to_block(else_block);
        }
        Operator::ReturnCallRef { type_index } => {
            // Get function signature
            // `index` is the index of the function's signature and `table_index` is the index of
            // the table to search the function in.
            let (sigref, num_args) =
                state.get_indirect_sig(builder.func, TypeIndex::from_u32(*type_index), env);
            let callee = state.pop1();

            // Bitcast any vector arguments to their default type, I8X16, before calling.
            let args = state.peekn_mut(num_args);
            bitcast_wasm_params(sigref, args, builder, env);

            env.translate_return_call_ref(builder, sigref, callee, state.peekn(num_args))?;

            state.popn(num_args);
            state.reachable = false;
        }

        // Garbage Collection
        // http://github.com/WebAssembly/gc
        Operator::RefI31 => {
            let val = state.pop1();
            let i31ref = env.translate_ref_i31(builder.cursor(), val)?;
            state.push1(i31ref);
        }
        Operator::I31GetS => {
            let i31ref = state.pop1();
            let val = env.translate_i31_get_s(builder.cursor(), i31ref)?;
            state.push1(val);
        }
        Operator::I31GetU => {
            let i31ref = state.pop1();
            let val = env.translate_i31_get_u(builder.cursor(), i31ref)?;
            state.push1(val);
        }
        Operator::StructNew { .. }
        | Operator::StructNewDefault { .. }
        | Operator::StructGet { .. }
        | Operator::StructGetS { .. }
        | Operator::StructGetU { .. }
        | Operator::StructSet { .. }
        | Operator::ArrayNew { .. }
        | Operator::ArrayNewDefault { .. }
        | Operator::ArrayNewFixed { .. }
        | Operator::ArrayNewData { .. }
        | Operator::ArrayNewElem { .. }
        | Operator::ArrayGet { .. }
        | Operator::ArrayGetS { .. }
        | Operator::ArrayGetU { .. }
        | Operator::ArraySet { .. }
        | Operator::ArrayLen
        | Operator::ArrayFill { .. }
        | Operator::ArrayCopy { .. }
        | Operator::ArrayInitData { .. }
        | Operator::ArrayInitElem { .. }
        | Operator::RefTestNonNull { .. }
        | Operator::RefTestNullable { .. }
        | Operator::RefCastNonNull { .. }
        | Operator::RefCastNullable { .. }
        | Operator::BrOnCast { .. }
        | Operator::BrOnCastFail { .. }
        | Operator::AnyConvertExtern
        | Operator::ExternConvertAny
        | Operator::RefEq => {
            return Err(wasm_unsupported!("Garbage Collection Proposal"));
        }

        /******************************* Active Proposals *****************************************/
        // memory control (experimental)
        // https://github.com/WebAssembly/memory-control
        Operator::MemoryDiscard { .. } => {
            return Err(wasm_unsupported!(
                "proposed memory-control operator {:?}",
                op
            ));
        }

        // shared-everything threads
        // https://github.com/WebAssembly/shared-everything-threads
        Operator::GlobalAtomicGet { .. }
        | Operator::GlobalAtomicSet { .. }
        | Operator::GlobalAtomicRmwAdd { .. }
        | Operator::GlobalAtomicRmwSub { .. }
        | Operator::GlobalAtomicRmwAnd { .. }
        | Operator::GlobalAtomicRmwOr { .. }
        | Operator::GlobalAtomicRmwXor { .. }
        | Operator::GlobalAtomicRmwXchg { .. }
        | Operator::GlobalAtomicRmwCmpxchg { .. }
        | Operator::TableAtomicGet { .. }
        | Operator::TableAtomicSet { .. }
        | Operator::TableAtomicRmwXchg { .. }
        | Operator::TableAtomicRmwCmpxchg { .. }
        | Operator::StructAtomicGet { .. }
        | Operator::StructAtomicGetS { .. }
        | Operator::StructAtomicGetU { .. }
        | Operator::StructAtomicSet { .. }
        | Operator::StructAtomicRmwAdd { .. }
        | Operator::StructAtomicRmwSub { .. }
        | Operator::StructAtomicRmwAnd { .. }
        | Operator::StructAtomicRmwOr { .. }
        | Operator::StructAtomicRmwXor { .. }
        | Operator::StructAtomicRmwXchg { .. }
        | Operator::StructAtomicRmwCmpxchg { .. }
        | Operator::ArrayAtomicGet { .. }
        | Operator::ArrayAtomicGetS { .. }
        | Operator::ArrayAtomicGetU { .. }
        | Operator::ArrayAtomicSet { .. }
        | Operator::ArrayAtomicRmwAdd { .. }
        | Operator::ArrayAtomicRmwSub { .. }
        | Operator::ArrayAtomicRmwAnd { .. }
        | Operator::ArrayAtomicRmwOr { .. }
        | Operator::ArrayAtomicRmwXor { .. }
        | Operator::ArrayAtomicRmwXchg { .. }
        | Operator::ArrayAtomicRmwCmpxchg { .. }
        | Operator::RefI31Shared => {
            return Err(wasm_unsupported!("Shared-Everything Threads Proposal"));
        }

        // Exception handling
        // https://github.com/WebAssembly/exception-handling
        Operator::TryTable { .. } | Operator::Throw { .. } | Operator::ThrowRef => {
            return Err(wasm_unsupported!("Exception Handling Proposal"));
        }
        // Deprecated old instructions from the exceptions proposal
        Operator::Try { .. }
        | Operator::Catch { .. }
        | Operator::Rethrow { .. }
        | Operator::Delegate { .. }
        | Operator::CatchAll => {
            return Err(wasm_unsupported!("Legacy Exception Handling Proposal"));
        }

        // Stack switching
        // https://github.com/WebAssembly/stack-switching
        Operator::ContNew { .. }
        | Operator::ContBind { .. }
        | Operator::Suspend { .. }
        | Operator::Resume { .. }
        | Operator::ResumeThrow { .. }
        | Operator::Switch { .. } => {
            return Err(wasm_unsupported!("Stack Switching Proposal"));
        }

        // wide arithmetic
        // https://github.com/WebAssembly/wide-arithmetic
        Operator::I64Add128
        | Operator::I64Sub128
        | Operator::I64MulWideS
        | Operator::I64MulWideU => {
            return Err(wasm_unsupported!("Wide Arithmetic Proposal"));
        }
    }

    Ok(())
}

/// Deals with a Wasm instruction located in an unreachable portion of the code. Most of them
/// are dropped but special ones like `End` or `Else` signal the potential end of the unreachable
/// portion so the translation state must be updated accordingly.
fn translate_unreachable_operator(
    validator: &FuncValidator<impl WasmModuleResources>,
    op: &Operator,
    builder: &mut FunctionBuilder,
    state: &mut FuncTranslationState,
    env: &mut TranslationEnvironment,
) {
    debug_assert!(!state.reachable);
    match *op {
        Operator::If { blockty } => {
            // Push a placeholder control stack entry. The if isn't reachable,
            // so we don't have any branches anywhere.
            state.push_if(
                ir::Block::reserved_value(),
                ElseData::NoElse {
                    branch_inst: ir::Inst::reserved_value(),
                    placeholder: ir::Block::reserved_value(),
                },
                0,
                0,
                blockty,
            );
        }
        Operator::Loop { blockty: _ } | Operator::Block { blockty: _ } => {
            state.push_block(ir::Block::reserved_value(), 0, 0);
        }
        Operator::Else => {
            let i = state.control_stack.len().checked_sub(1).unwrap();
            match state.control_stack[i] {
                ControlStackFrame::If {
                    ref else_data,
                    head_is_reachable,
                    ref mut consequent_ends_reachable,
                    blocktype,
                    ..
                } => {
                    debug_assert!(consequent_ends_reachable.is_none());
                    *consequent_ends_reachable = Some(state.reachable);

                    if head_is_reachable {
                        // We have a branch from the head of the `if` to the `else`.
                        state.reachable = true;

                        let else_block = match *else_data {
                            ElseData::NoElse {
                                branch_inst,
                                placeholder,
                            } => {
                                let (params, _results) =
                                    blocktype_params_results(validator, blocktype);
                                let else_block = block_with_params(builder, params, env);
                                let frame = state.control_stack.last().unwrap();
                                frame.truncate_value_stack_to_else_params(&mut state.stack);

                                // We change the target of the branch instruction.
                                builder.change_jump_destination(
                                    branch_inst,
                                    placeholder,
                                    else_block,
                                );
                                builder.seal_block(else_block);
                                else_block
                            }
                            ElseData::WithElse { else_block } => {
                                let frame = state.control_stack.last().unwrap();
                                frame.truncate_value_stack_to_else_params(&mut state.stack);
                                else_block
                            }
                        };

                        builder.switch_to_block(else_block);

                        // Again, no need to push the parameters for the `else`,
                        // since we already did when we saw the original `if`. See
                        // the comment for translating `Operator::Else` in
                        // `translate_operator` for details.
                    }
                }
                _ => unreachable!(),
            }
        }
        Operator::End => {
            let stack = &mut state.stack;
            let control_stack = &mut state.control_stack;
            let frame = control_stack.pop().unwrap();

            // Pop unused parameters from stack.
            frame.truncate_value_stack_to_original_size(stack);

            let reachable_anyway = match frame {
                // If it is a loop we also have to seal the body loop block
                ControlStackFrame::Loop { header, .. } => {
                    builder.seal_block(header);
                    // And loops can't have branches to the end.
                    false
                }
                // Since we are only in this function when in unreachable code,
                // we know that the alternative just ended unreachable. Whether
                // the following block is reachable depends on if the consequent
                // ended reachable or not.
                ControlStackFrame::If {
                    head_is_reachable,
                    consequent_ends_reachable: Some(consequent_ends_reachable),
                    ..
                } => head_is_reachable && consequent_ends_reachable,
                // If we never set `consequent_ends_reachable` then that means
                // we are finishing the consequent now, and there was no
                // `else`. Whether the following block is reachable depends only
                // on if the head was reachable.
                ControlStackFrame::If {
                    head_is_reachable,
                    consequent_ends_reachable: None,
                    ..
                } => head_is_reachable,
                // All other control constructs are already handled.
                _ => false,
            };

            if frame.exit_is_branched_to() || reachable_anyway {
                builder.switch_to_block(frame.following_code());
                builder.seal_block(frame.following_code());

                // And add the return values of the block but only if the next block is reachable
                // (which corresponds to testing if the stack depth is 1)
                stack.extend_from_slice(builder.block_params(frame.following_code()));
                state.reachable = true;
            }
        }
        _ => {
            // We don't parse because this is unreachable code
        }
    }
}

/// Translate a load instruction.
///
/// Returns the execution state's reachability after the load is translated.
fn translate_load(
    memarg: &MemArg,
    opcode: ir::Opcode,
    result_ty: Type,
    builder: &mut FunctionBuilder,
    state: &mut FuncTranslationState,
    env: &mut TranslationEnvironment,
) -> Reachability<()> {
    let memory_index = MemoryIndex::from_u32(memarg.memory);
    let index = state.pop1();
    let mem_op_size = mem_op_size(opcode, result_ty);

    let mem = state.get_memory(builder.func, memory_index, env);
    let (flags, _wasm_index, base) =
        match mem.prepare_addr(builder, index, mem_op_size, memarg, env) {
            Reachability::Unreachable => return Reachability::Unreachable,
            Reachability::Reachable((f, i, b)) => (f, i, b),
        };

    let (load, dfg) = builder
        .ins()
        .Load(opcode, result_ty, flags, Offset32::new(0), base);
    state.push1(dfg.first_result(load));

    Reachability::Reachable(())
}

/// Translate a store instruction.
fn translate_store(
    memarg: &MemArg,
    opcode: ir::Opcode,
    builder: &mut FunctionBuilder,
    state: &mut FuncTranslationState,
    env: &mut TranslationEnvironment,
) -> crate::wasm::Result<()> {
    let memory_index = MemoryIndex::from_u32(memarg.memory);
    let val = state.pop1();
    let index = state.pop1();
    let val_ty = builder.func.dfg.value_type(val);
    let mem_op_size = mem_op_size(opcode, val_ty);

    let mem = state.get_memory(builder.func, memory_index, env);
    let (flags, _wasm_index, base) = unwrap_or_return_unreachable_state!(
        state,
        mem.prepare_addr(builder, index, mem_op_size, memarg, env)
    );

    builder
        .ins()
        .Store(opcode, val_ty, flags, Offset32::new(0), val, base);
    Ok(())
}

fn translate_atomic_rmw(
    _widened_ty: Type,
    _access_ty: Type,
    _op: AtomicRmwOp,
    _memarg: &MemArg,
    _builder: &mut FunctionBuilder,
    _state: &mut FuncTranslationState,
    _env: &mut TranslationEnvironment,
) -> crate::wasm::Result<()> {
    todo!()
}

fn translate_atomic_cas(
    _widened_ty: Type,
    _access_ty: Type,
    _memarg: &MemArg,
    _builder: &mut FunctionBuilder,
    _state: &mut FuncTranslationState,
    _env: &mut TranslationEnvironment,
) -> crate::wasm::Result<()> {
    todo!()
}

fn translate_atomic_load(
    _widened_ty: Type,
    _access_ty: Type,
    _memarg: &MemArg,
    _builder: &mut FunctionBuilder,
    _state: &mut FuncTranslationState,
    _env: &mut TranslationEnvironment,
) -> crate::wasm::Result<()> {
    todo!()
}

fn translate_atomic_store(
    _access_ty: Type,
    _memarg: &MemArg,
    _builder: &mut FunctionBuilder,
    _state: &mut FuncTranslationState,
    _env: &mut TranslationEnvironment,
) -> crate::wasm::Result<()> {
    todo!()
}

fn mem_op_size(opcode: ir::Opcode, ty: Type) -> u8 {
    match opcode {
        ir::Opcode::Istore8 | ir::Opcode::Sload8 | ir::Opcode::Uload8 => 1,
        ir::Opcode::Istore16 | ir::Opcode::Sload16 | ir::Opcode::Uload16 => 2,
        ir::Opcode::Istore32 | ir::Opcode::Sload32 | ir::Opcode::Uload32 => 4,
        ir::Opcode::Store | ir::Opcode::Load => u8::try_from(ty.bytes()).unwrap(),
        _ => panic!("unknown size of mem op for {opcode:?}"),
    }
}

fn translate_icmp(cc: IntCC, builder: &mut FunctionBuilder, state: &mut FuncTranslationState) {
    let (arg0, arg1) = state.pop2();
    let val = builder.ins().icmp(cc, arg0, arg1);
    state.push1(builder.ins().uextend(I32, val));
}

fn translate_vector_icmp(
    cc: IntCC,
    needed_type: Type,
    builder: &mut FunctionBuilder,
    state: &mut FuncTranslationState,
) {
    let (a, b) = state.pop2();
    let bitcast_a = optionally_bitcast_vector(a, needed_type, builder);
    let bitcast_b = optionally_bitcast_vector(b, needed_type, builder);
    state.push1(builder.ins().icmp(cc, bitcast_a, bitcast_b));
}

fn translate_fcmp(cc: FloatCC, builder: &mut FunctionBuilder, state: &mut FuncTranslationState) {
    let (arg0, arg1) = state.pop2();
    let val = builder.ins().fcmp(cc, arg0, arg1);
    state.push1(builder.ins().uextend(I32, val));
}

fn translate_vector_fcmp(
    cc: FloatCC,
    needed_type: Type,
    builder: &mut FunctionBuilder,
    state: &mut FuncTranslationState,
) {
    let (a, b) = state.pop2();
    let bitcast_a = optionally_bitcast_vector(a, needed_type, builder);
    let bitcast_b = optionally_bitcast_vector(b, needed_type, builder);
    state.push1(builder.ins().fcmp(cc, bitcast_a, bitcast_b));
}

fn translate_br_if(
    relative_depth: u32,
    builder: &mut FunctionBuilder,
    state: &mut FuncTranslationState,
) {
    let val = state.pop1();
    let (br_destination, inputs) = translate_br_if_args(relative_depth, state);
    let next_block = builder.create_block();
    canonicalise_brif(builder, val, br_destination, inputs, next_block, &[]);

    builder.seal_block(next_block); // The only predecessor is the current block.
    builder.switch_to_block(next_block);
}

fn translate_br_if_args(
    relative_depth: u32,
    state: &mut FuncTranslationState,
) -> (ir::Block, &mut [Value]) {
    // FIXME fix this ugly mess
    let i = state
        .control_stack
        .len()
        .checked_sub(1)
        .unwrap()
        .checked_sub(usize::try_from(relative_depth).unwrap())
        .unwrap();

    let (return_count, br_destination) = {
        let frame = &mut state.control_stack[i];
        // The values returned by the branch are still available for the reachable
        // code that comes after it
        frame.set_branched_to_exit();
        let return_count = if frame.is_loop() {
            frame.num_param_values()
        } else {
            frame.num_return_values()
        };
        (return_count, frame.br_destination())
    };
    let inputs = state.peekn_mut(return_count);
    (br_destination, inputs)
}

/// Determine the returned value type of a WebAssembly operator
fn type_of(operator: &Operator) -> Type {
    match operator {
        Operator::V128Load { .. }
        | Operator::V128Store { .. }
        | Operator::V128Const { .. }
        | Operator::V128Not
        | Operator::V128And
        | Operator::V128AndNot
        | Operator::V128Or
        | Operator::V128Xor
        | Operator::V128AnyTrue
        | Operator::V128Bitselect => I8X16, // default type representing V128

        Operator::I8x16Shuffle { .. }
        | Operator::I8x16Splat
        | Operator::V128Load8Splat { .. }
        | Operator::V128Load8Lane { .. }
        | Operator::V128Store8Lane { .. }
        | Operator::I8x16ExtractLaneS { .. }
        | Operator::I8x16ExtractLaneU { .. }
        | Operator::I8x16ReplaceLane { .. }
        | Operator::I8x16Eq
        | Operator::I8x16Ne
        | Operator::I8x16LtS
        | Operator::I8x16LtU
        | Operator::I8x16GtS
        | Operator::I8x16GtU
        | Operator::I8x16LeS
        | Operator::I8x16LeU
        | Operator::I8x16GeS
        | Operator::I8x16GeU
        | Operator::I8x16Neg
        | Operator::I8x16Abs
        | Operator::I8x16AllTrue
        | Operator::I8x16Shl
        | Operator::I8x16ShrS
        | Operator::I8x16ShrU
        | Operator::I8x16Add
        | Operator::I8x16AddSatS
        | Operator::I8x16AddSatU
        | Operator::I8x16Sub
        | Operator::I8x16SubSatS
        | Operator::I8x16SubSatU
        | Operator::I8x16MinS
        | Operator::I8x16MinU
        | Operator::I8x16MaxS
        | Operator::I8x16MaxU
        | Operator::I8x16AvgrU
        | Operator::I8x16Bitmask
        | Operator::I8x16Popcnt
        | Operator::I8x16RelaxedLaneselect => I8X16,

        Operator::I16x8Splat
        | Operator::V128Load16Splat { .. }
        | Operator::V128Load16Lane { .. }
        | Operator::V128Store16Lane { .. }
        | Operator::I16x8ExtractLaneS { .. }
        | Operator::I16x8ExtractLaneU { .. }
        | Operator::I16x8ReplaceLane { .. }
        | Operator::I16x8Eq
        | Operator::I16x8Ne
        | Operator::I16x8LtS
        | Operator::I16x8LtU
        | Operator::I16x8GtS
        | Operator::I16x8GtU
        | Operator::I16x8LeS
        | Operator::I16x8LeU
        | Operator::I16x8GeS
        | Operator::I16x8GeU
        | Operator::I16x8Neg
        | Operator::I16x8Abs
        | Operator::I16x8AllTrue
        | Operator::I16x8Shl
        | Operator::I16x8ShrS
        | Operator::I16x8ShrU
        | Operator::I16x8Add
        | Operator::I16x8AddSatS
        | Operator::I16x8AddSatU
        | Operator::I16x8Sub
        | Operator::I16x8SubSatS
        | Operator::I16x8SubSatU
        | Operator::I16x8MinS
        | Operator::I16x8MinU
        | Operator::I16x8MaxS
        | Operator::I16x8MaxU
        | Operator::I16x8AvgrU
        | Operator::I16x8Mul
        | Operator::I16x8Bitmask
        | Operator::I16x8RelaxedLaneselect => I16X8,

        Operator::I32x4Splat
        | Operator::V128Load32Splat { .. }
        | Operator::V128Load32Lane { .. }
        | Operator::V128Store32Lane { .. }
        | Operator::I32x4ExtractLane { .. }
        | Operator::I32x4ReplaceLane { .. }
        | Operator::I32x4Eq
        | Operator::I32x4Ne
        | Operator::I32x4LtS
        | Operator::I32x4LtU
        | Operator::I32x4GtS
        | Operator::I32x4GtU
        | Operator::I32x4LeS
        | Operator::I32x4LeU
        | Operator::I32x4GeS
        | Operator::I32x4GeU
        | Operator::I32x4Neg
        | Operator::I32x4Abs
        | Operator::I32x4AllTrue
        | Operator::I32x4Shl
        | Operator::I32x4ShrS
        | Operator::I32x4ShrU
        | Operator::I32x4Add
        | Operator::I32x4Sub
        | Operator::I32x4Mul
        | Operator::I32x4MinS
        | Operator::I32x4MinU
        | Operator::I32x4MaxS
        | Operator::I32x4MaxU
        | Operator::I32x4Bitmask
        | Operator::I32x4TruncSatF32x4S
        | Operator::I32x4TruncSatF32x4U
        | Operator::I32x4RelaxedLaneselect
        | Operator::V128Load32Zero { .. } => I32X4,

        Operator::I64x2Splat
        | Operator::V128Load64Splat { .. }
        | Operator::V128Load64Lane { .. }
        | Operator::V128Store64Lane { .. }
        | Operator::I64x2ExtractLane { .. }
        | Operator::I64x2ReplaceLane { .. }
        | Operator::I64x2Eq
        | Operator::I64x2Ne
        | Operator::I64x2LtS
        | Operator::I64x2GtS
        | Operator::I64x2LeS
        | Operator::I64x2GeS
        | Operator::I64x2Neg
        | Operator::I64x2Abs
        | Operator::I64x2AllTrue
        | Operator::I64x2Shl
        | Operator::I64x2ShrS
        | Operator::I64x2ShrU
        | Operator::I64x2Add
        | Operator::I64x2Sub
        | Operator::I64x2Mul
        | Operator::I64x2Bitmask
        | Operator::I64x2RelaxedLaneselect
        | Operator::V128Load64Zero { .. } => I64X2,

        Operator::F32x4Splat
        | Operator::F32x4ExtractLane { .. }
        | Operator::F32x4ReplaceLane { .. }
        | Operator::F32x4Eq
        | Operator::F32x4Ne
        | Operator::F32x4Lt
        | Operator::F32x4Gt
        | Operator::F32x4Le
        | Operator::F32x4Ge
        | Operator::F32x4Abs
        | Operator::F32x4Neg
        | Operator::F32x4Sqrt
        | Operator::F32x4Add
        | Operator::F32x4Sub
        | Operator::F32x4Mul
        | Operator::F32x4Div
        | Operator::F32x4Min
        | Operator::F32x4Max
        | Operator::F32x4PMin
        | Operator::F32x4PMax
        | Operator::F32x4ConvertI32x4S
        | Operator::F32x4ConvertI32x4U
        | Operator::F32x4Ceil
        | Operator::F32x4Floor
        | Operator::F32x4Trunc
        | Operator::F32x4Nearest
        | Operator::F32x4RelaxedMax
        | Operator::F32x4RelaxedMin
        | Operator::F32x4RelaxedMadd
        | Operator::F32x4RelaxedNmadd => F32X4,

        Operator::F64x2Splat
        | Operator::F64x2ExtractLane { .. }
        | Operator::F64x2ReplaceLane { .. }
        | Operator::F64x2Eq
        | Operator::F64x2Ne
        | Operator::F64x2Lt
        | Operator::F64x2Gt
        | Operator::F64x2Le
        | Operator::F64x2Ge
        | Operator::F64x2Abs
        | Operator::F64x2Neg
        | Operator::F64x2Sqrt
        | Operator::F64x2Add
        | Operator::F64x2Sub
        | Operator::F64x2Mul
        | Operator::F64x2Div
        | Operator::F64x2Min
        | Operator::F64x2Max
        | Operator::F64x2PMin
        | Operator::F64x2PMax
        | Operator::F64x2Ceil
        | Operator::F64x2Floor
        | Operator::F64x2Trunc
        | Operator::F64x2Nearest
        | Operator::F64x2RelaxedMax
        | Operator::F64x2RelaxedMin
        | Operator::F64x2RelaxedMadd
        | Operator::F64x2RelaxedNmadd => F64X2,

        _ => unimplemented!(
            "Currently only SIMD instructions are mapped to their return type; the \
             following instruction is not mapped: {:?}",
            operator
        ),
    }
}

/// Some SIMD operations only operate on I8X16 in CLIF; this will convert them to that type by
/// adding a bitcast if necessary.
fn optionally_bitcast_vector(
    value: Value,
    needed_type: Type,
    builder: &mut FunctionBuilder,
) -> Value {
    if builder.func.dfg.value_type(value) != needed_type {
        let mut flags = MemFlags::new();
        flags.set_endianness(ir::Endianness::Little);
        builder.ins().bitcast(needed_type, flags, value)
    } else {
        value
    }
}

#[inline(always)]
fn is_non_canonical_v128(ty: Type) -> bool {
    matches!(ty, I64X2 | I32X4 | I16X8 | F32X4 | F64X2)
}

/// Cast to I8X16, any vector values in `values` that are of "non-canonical" type (meaning, not
/// I8X16), and return them in a slice.  A pre-scan is made to determine whether any casts are
/// actually necessary, and if not, the original slice is returned.  Otherwise the cast values
/// are returned in a slice that belongs to the caller-supplied `SmallVec`.
fn canonicalise_v128_values<'a>(
    tmp_canonicalised: &'a mut SmallVec<[Value; 16]>,
    builder: &mut FunctionBuilder,
    values: &'a [Value],
) -> &'a [Value] {
    debug_assert!(tmp_canonicalised.is_empty());
    // First figure out if any of the parameters need to be cast.  Mostly they don't need to be.
    let any_non_canonical = values
        .iter()
        .any(|v| is_non_canonical_v128(builder.func.dfg.value_type(*v)));
    // Hopefully we take this exit most of the time, hence doing no mem allocation.
    if !any_non_canonical {
        return values;
    }
    // Otherwise we'll have to cast, and push the resulting `Value`s into `canonicalised`.
    for v in values {
        tmp_canonicalised.push(if is_non_canonical_v128(builder.func.dfg.value_type(*v)) {
            let mut flags = MemFlags::new();
            flags.set_endianness(ir::Endianness::Little);
            builder.ins().bitcast(I8X16, flags, *v)
        } else {
            *v
        });
    }
    tmp_canonicalised.as_slice()
}

/// Generate a `jump` instruction, but first cast all 128-bit vector values to I8X16 if they
/// don't have that type.  This is done in somewhat roundabout way so as to ensure that we
/// almost never have to do any mem allocation.
fn canonicalise_then_jump(
    builder: &mut FunctionBuilder,
    destination: ir::Block,
    params: &[Value],
) -> ir::Inst {
    let mut tmp_canonicalised = SmallVec::<[Value; 16]>::new();
    let canonicalised = canonicalise_v128_values(&mut tmp_canonicalised, builder, params);
    builder.ins().jump(destination, canonicalised)
}

/// The same but for a `brif` instruction.
fn canonicalise_brif(
    builder: &mut FunctionBuilder,
    cond: Value,
    block_then: ir::Block,
    params_then: &[Value],
    block_else: ir::Block,
    params_else: &[Value],
) -> ir::Inst {
    let mut tmp_canonicalised_then = SmallVec::<[Value; 16]>::new();
    let canonicalised_then =
        canonicalise_v128_values(&mut tmp_canonicalised_then, builder, params_then);
    let mut tmp_canonicalised_else = SmallVec::<[Value; 16]>::new();
    let canonicalised_else =
        canonicalise_v128_values(&mut tmp_canonicalised_else, builder, params_else);
    builder.ins().brif(
        cond,
        block_then,
        canonicalised_then,
        block_else,
        canonicalised_else,
    )
}

/// A helper for popping and bitcasting a single value; since SIMD values can lose their type by
/// using v128 (i.e. CLIF's I8x16) we must re-type the values using a bitcast to avoid CLIF
/// typing issues.
fn pop1_with_bitcast(
    state: &mut FuncTranslationState,
    needed_type: Type,
    builder: &mut FunctionBuilder,
) -> Value {
    optionally_bitcast_vector(state.pop1(), needed_type, builder)
}

/// A helper for popping and bitcasting two values; since SIMD values can lose their type by
/// using v128 (i.e. CLIF's I8x16) we must re-type the values using a bitcast to avoid CLIF
/// typing issues.
fn pop2_with_bitcast(
    state: &mut FuncTranslationState,
    needed_type: Type,
    builder: &mut FunctionBuilder,
) -> (Value, Value) {
    let (a, b) = state.pop2();
    let bitcast_a = optionally_bitcast_vector(a, needed_type, builder);
    let bitcast_b = optionally_bitcast_vector(b, needed_type, builder);
    (bitcast_a, bitcast_b)
}

fn pop3_with_bitcast(
    state: &mut FuncTranslationState,
    needed_type: Type,
    builder: &mut FunctionBuilder,
) -> (Value, Value, Value) {
    let (a, b, c) = state.pop3();
    let bitcast_a = optionally_bitcast_vector(a, needed_type, builder);
    let bitcast_b = optionally_bitcast_vector(b, needed_type, builder);
    let bitcast_c = optionally_bitcast_vector(c, needed_type, builder);
    (bitcast_a, bitcast_b, bitcast_c)
}

fn bitcast_arguments<'a>(
    builder: &FunctionBuilder,
    arguments: &'a mut [Value],
    params: &[ir::AbiParam],
    param_predicate: impl Fn(usize) -> bool,
) -> Vec<(Type, &'a mut Value)> {
    let filtered_param_types = params
        .iter()
        .enumerate()
        .filter(|(i, _)| param_predicate(*i))
        .map(|(_, param)| param.value_type);

    let pairs = filtered_param_types.zip_eq(arguments).unwrap();

    // let pairs = ZipEq {
    //     a: filtered_param_types,
    //     b: arguments.iter_mut(),
    // };

    // The arguments which need to be bitcasted are those which have some vector type but the type
    // expected by the parameter is not the same vector type as that of the provided argument.
    pairs
        .filter(|(param_type, _)| param_type.is_vector())
        .filter(|(param_type, arg)| {
            let arg_type = builder.func.dfg.value_type(**arg);
            assert!(
                arg_type.is_vector(),
                "unexpected type mismatch: expected {}, argument {} was actually of type {}",
                param_type,
                *arg,
                arg_type
            );

            // This is the same check that would be done by `optionally_bitcast_vector`, except we
            // can't take a mutable borrow of the FunctionBuilder here, so we defer inserting the
            // bitcast instruction to the caller.
            arg_type != *param_type
        })
        .collect()
}

/// A helper for bitcasting a sequence of return values for the function currently being built. If
/// a value is a vector type that does not match its expected type, this will modify the value in
/// place to point to the result of a `bitcast`. This conversion is necessary to parse Wasm
/// code that uses `V128` as function parameters (or implicitly in block parameters) and still use
/// specific CLIF types (e.g. `I32X4`) in the function body.
pub fn bitcast_wasm_returns(
    arguments: &mut [Value],
    builder: &mut FunctionBuilder,
    env: &mut TranslationEnvironment,
) {
    let changes = bitcast_arguments(builder, arguments, &builder.func.signature.returns, |i| {
        env.is_wasm_return(&builder.func.signature, i)
    });
    for (t, arg) in changes {
        let mut flags = MemFlags::new();
        flags.set_endianness(ir::Endianness::Little);
        *arg = builder.ins().bitcast(t, flags, *arg);
    }
}

/// Like `bitcast_wasm_returns`, but for the parameters being passed to a specified callee.
fn bitcast_wasm_params(
    callee_signature: ir::SigRef,
    arguments: &mut [Value],
    builder: &mut FunctionBuilder,
    env: &mut TranslationEnvironment,
) {
    let callee_signature = &builder.func.dfg.signatures[callee_signature];
    let changes = bitcast_arguments(builder, arguments, &callee_signature.params, |i| {
        env.is_wasm_parameter(i)
    });
    for (t, arg) in changes {
        let mut flags = MemFlags::new();
        flags.set_endianness(ir::Endianness::Little);
        *arg = builder.ins().bitcast(t, flags, *arg);
    }
}
