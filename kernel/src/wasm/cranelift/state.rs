// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::vec::Vec;

use cranelift_codegen::ir;
use cranelift_codegen::ir::{Block, FuncRef, Function, Inst, SigRef, Value};
use hashbrown::HashMap;

use crate::wasm::cranelift::env::TranslationEnvironment;
use crate::wasm::cranelift::memory::CraneliftMemory;
use crate::wasm::cranelift::{CraneliftGlobal, CraneliftTable};
use crate::wasm::indices::{FuncIndex, GlobalIndex, MemoryIndex, TableIndex, TypeIndex};

pub struct FuncTranslationState {
    /// A stack of values corresponding to the active values in the input wasm function at this
    /// point.
    pub(crate) stack: Vec<Value>,
    /// A stack of active control flow operations at this point in the input wasm function.
    pub(crate) control_stack: Vec<ControlStackFrame>,
    /// Is the current translation state still reachable? This is false when translating operators
    /// like End, Return, or Unreachable.
    pub(crate) reachable: bool,
    /// Indirect call signatures that have been created by
    /// `FuncEnvironment::get_indirect_sig()`.
    /// Stores both the signature reference and the number of WebAssembly arguments
    signatures: HashMap<TypeIndex, (SigRef, usize)>,
    /// Imported and local functions that have been crated
    /// through `TranslationEnvironment::get_direct_func`.
    /// Stores both the function reference and the number of WebAssembly arguments
    functions: HashMap<FuncIndex, (FuncRef, usize)>,
    tables: HashMap<TableIndex, CraneliftTable>,
    memories: HashMap<MemoryIndex, CraneliftMemory>,
    globals: HashMap<GlobalIndex, CraneliftGlobal>,
}

impl FuncTranslationState {
    /// Construct a new, empty, `FuncTranslationState`
    pub(crate) fn new() -> Self {
        Self {
            stack: Vec::new(),
            control_stack: Vec::new(),
            reachable: true,
            signatures: HashMap::default(),
            functions: HashMap::default(),
            tables: HashMap::default(),
            memories: HashMap::default(),
            globals: HashMap::default(),
        }
    }

    pub(crate) fn clear(&mut self) {
        debug_assert!(self.stack.is_empty());
        debug_assert!(self.control_stack.is_empty());
        self.reachable = true;
        self.signatures.clear();
        self.functions.clear();
        self.tables.clear();
        self.memories.clear();
        self.globals.clear();
    }

    /// Initialize the state for compiling a function with the given signature.
    ///
    /// This resets the state to containing only a single block representing the whole function.
    /// The exit block is the last block in the function which will contain the return instruction.
    pub(crate) fn initialize(&mut self, sig: &ir::Signature, exit_block: Block) {
        self.clear();
        self.push_block(
            exit_block,
            0,
            sig.returns
                .iter()
                .filter(|arg| arg.purpose == ir::ArgumentPurpose::Normal)
                .count(),
        );
    }

    /// Push a value.
    pub(crate) fn push1(&mut self, val: Value) {
        self.stack.push(val);
    }

    /// Push multiple values.
    pub(crate) fn pushn(&mut self, vals: &[Value]) {
        self.stack.extend_from_slice(vals);
    }

    /// Pop one value.
    pub(crate) fn pop1(&mut self) -> Value {
        self.stack
            .pop()
            .expect("attempted to pop a value from an empty stack")
    }

    /// Peek at the top of the stack without popping it.
    pub(crate) fn peek1(&self) -> Value {
        *self
            .stack
            .last()
            .expect("attempted to peek at a value on an empty stack")
    }

    /// Pop two values. Return them in the order they were pushed.
    pub(crate) fn pop2(&mut self) -> (Value, Value) {
        let v2 = self.stack.pop().unwrap();
        let v1 = self.stack.pop().unwrap();
        (v1, v2)
    }

    /// Pop three values. Return them in the order they were pushed.
    pub(crate) fn pop3(&mut self) -> (Value, Value, Value) {
        let v3 = self.stack.pop().unwrap();
        let v2 = self.stack.pop().unwrap();
        let v1 = self.stack.pop().unwrap();
        (v1, v2, v3)
    }

    /// Pop the top `n` values on the stack.
    ///
    /// The popped values are not returned. Use `peekn` to look at them before popping.
    pub(crate) fn popn(&mut self, n: usize) {
        self.ensure_length_is_at_least(n);
        let new_len = self.stack.len().wrapping_sub(n);
        self.stack.truncate(new_len);
    }

    /// Peek at the top `n` values on the stack in the order they were pushed.
    pub(crate) fn peekn(&self, n: usize) -> &[Value] {
        self.ensure_length_is_at_least(n);
        &self.stack[self.stack.len().wrapping_sub(n)..]
    }

    /// Peek at the top `n` values on the stack in the order they were pushed.
    pub(crate) fn peekn_mut(&mut self, n: usize) -> &mut [Value] {
        self.ensure_length_is_at_least(n);
        let len = self.stack.len();
        &mut self.stack[len.wrapping_sub(n)..]
    }

    /// Push a block on the control stack.
    pub(crate) fn push_block(
        &mut self,
        following_code: Block,
        num_param_types: usize,
        num_result_types: usize,
    ) {
        debug_assert!(num_param_types <= self.stack.len());
        self.control_stack.push(ControlStackFrame::Block {
            destination: following_code,
            original_stack_size: self.stack.len().wrapping_sub(num_param_types),
            num_param_values: num_param_types,
            num_return_values: num_result_types,
            exit_is_branched_to: false,
        });
    }

    /// Push a loop on the control stack.
    pub(crate) fn push_loop(
        &mut self,
        header: Block,
        following_code: Block,
        num_param_types: usize,
        num_result_types: usize,
    ) {
        debug_assert!(num_param_types <= self.stack.len());
        self.control_stack.push(ControlStackFrame::Loop {
            header,
            destination: following_code,
            original_stack_size: self.stack.len().wrapping_sub(num_param_types),
            num_param_values: num_param_types,
            num_return_values: num_result_types,
        });
    }

    /// Push an if on the control stack.
    pub(crate) fn push_if(
        &mut self,
        destination: Block,
        else_data: ElseData,
        num_param_types: usize,
        num_result_types: usize,
        blocktype: wasmparser::BlockType,
    ) {
        debug_assert!(num_param_types <= self.stack.len());

        // Push a second copy of our `if`'s parameters on the stack. This lets
        // us avoid saving them on the side in the `ControlStackFrame` for our
        // `else` block (if it exists), which would require a second heap
        // allocation. See also the comment in `translate_operator` for
        // `Operator::Else`.
        self.stack.reserve(num_param_types);
        for i in self.stack.len().wrapping_sub(num_param_types)..self.stack.len() {
            let val = self.stack[i];
            self.stack.push(val);
        }

        self.control_stack.push(ControlStackFrame::If {
            destination,
            else_data,
            original_stack_size: self.stack.len().wrapping_sub(num_param_types),
            num_param_values: num_param_types,
            num_return_values: num_result_types,
            exit_is_branched_to: false,
            head_is_reachable: self.reachable,
            consequent_ends_reachable: None,
            blocktype,
        });
    }

    pub(crate) fn get_direct_func(
        &mut self,
        func: &mut Function,
        index: FuncIndex,
        env: &mut TranslationEnvironment,
    ) -> (FuncRef, usize) {
        *self.functions.entry(index).or_insert_with(|| {
            let fref = env.make_direct_func(func, index);
            let sig = func.dfg.ext_funcs[fref].signature;
            (fref, num_wasm_parameters(&func.dfg.signatures[sig], env))
        })
    }

    pub(crate) fn get_indirect_sig(
        &mut self,
        func: &mut Function,
        index: TypeIndex,
        env: &mut TranslationEnvironment,
    ) -> (SigRef, usize) {
        *self.signatures.entry(index).or_insert_with(|| {
            let sig = env.make_indirect_sig(func, index);
            (sig, num_wasm_parameters(&func.dfg.signatures[sig], env))
        })
    }

    pub(crate) fn get_global(
        &mut self,
        func: &mut Function,
        index: GlobalIndex,
        env: &mut TranslationEnvironment,
    ) -> &'_ mut CraneliftGlobal {
        self.globals
            .entry(index)
            .or_insert_with(|| env.make_global(func, index))
    }

    pub(crate) fn get_memory(
        &mut self,
        func: &mut Function,
        index: MemoryIndex,
        env: &mut TranslationEnvironment,
    ) -> &CraneliftMemory {
        self.memories
            .entry(index)
            .or_insert_with(|| env.make_memory(func, index))
    }

    pub(crate) fn get_table(
        &mut self,
        func: &mut Function,
        index: TableIndex,
        env: &mut TranslationEnvironment,
    ) -> &'_ mut CraneliftTable {
        self.tables
            .entry(index)
            .or_insert_with(|| env.make_table(func, index))
    }

    #[inline]
    fn ensure_length_is_at_least(&self, n: usize) {
        debug_assert!(
            n <= self.stack.len(),
            "attempted to access {} values but stack only has {} values",
            n,
            self.stack.len()
        );
    }
}

/// A control stack frame can be an `if`, a `block` or a `loop`, each one having the following
/// fields:
///
/// - `destination`: reference to the `Block` that will hold the code after the control block;
/// - `num_return_values`: number of values returned by the control block;
/// - `original_stack_size`: size of the value stack at the beginning of the control block.
///
/// The `loop` frame has a `header` field that references the `Block` that contains the beginning
/// of the body of the loop.
#[derive(Debug)]
pub enum ControlStackFrame {
    If {
        destination: Block,
        else_data: ElseData,
        num_param_values: usize,
        num_return_values: usize,
        original_stack_size: usize,
        exit_is_branched_to: bool,
        blocktype: wasmparser::BlockType,
        /// Was the head of the `if` reachable?
        head_is_reachable: bool,
        /// What was the reachability at the end of the consequent?
        ///
        /// This is `None` until we're finished translating the consequent, and
        /// is set to `Some` either by hitting an `else` when we will begin
        /// translating the alternative, or by hitting an `end` in which case
        /// there is no alternative.
        consequent_ends_reachable: Option<bool>,
        // Note: no need for `alternative_ends_reachable` because that is just
        // `state.reachable` when we hit the `end` in the `if .. else .. end`.
    },
    Block {
        destination: Block,
        num_param_values: usize,
        num_return_values: usize,
        original_stack_size: usize,
        exit_is_branched_to: bool,
    },
    Loop {
        destination: Block,
        header: Block,
        num_param_values: usize,
        num_return_values: usize,
        original_stack_size: usize,
    },
}

/// Helper methods for the control stack objects.
impl ControlStackFrame {
    pub fn num_return_values(&self) -> usize {
        match *self {
            Self::If {
                num_return_values, ..
            }
            | Self::Block {
                num_return_values, ..
            }
            | Self::Loop {
                num_return_values, ..
            } => num_return_values,
        }
    }
    pub fn num_param_values(&self) -> usize {
        match *self {
            Self::If {
                num_param_values, ..
            }
            | Self::Block {
                num_param_values, ..
            }
            | Self::Loop {
                num_param_values, ..
            } => num_param_values,
        }
    }
    pub fn following_code(&self) -> Block {
        match *self {
            Self::If { destination, .. }
            | Self::Block { destination, .. }
            | Self::Loop { destination, .. } => destination,
        }
    }
    pub fn br_destination(&self) -> Block {
        match *self {
            Self::If { destination, .. } | Self::Block { destination, .. } => destination,
            Self::Loop { header, .. } => header,
        }
    }
    /// Private helper. Use `truncate_value_stack_to_else_params()` or
    /// `truncate_value_stack_to_original_size()` to restore value-stack state.
    fn original_stack_size(&self) -> usize {
        match *self {
            Self::If {
                original_stack_size,
                ..
            }
            | Self::Block {
                original_stack_size,
                ..
            }
            | Self::Loop {
                original_stack_size,
                ..
            } => original_stack_size,
        }
    }
    pub fn is_loop(&self) -> bool {
        match *self {
            Self::If { .. } | Self::Block { .. } => false,
            Self::Loop { .. } => true,
        }
    }

    pub fn exit_is_branched_to(&self) -> bool {
        match *self {
            Self::If {
                exit_is_branched_to,
                ..
            }
            | Self::Block {
                exit_is_branched_to,
                ..
            } => exit_is_branched_to,
            Self::Loop { .. } => false,
        }
    }

    pub fn set_branched_to_exit(&mut self) {
        match *self {
            Self::If {
                ref mut exit_is_branched_to,
                ..
            }
            | Self::Block {
                ref mut exit_is_branched_to,
                ..
            } => *exit_is_branched_to = true,
            Self::Loop { .. } => {}
        }
    }

    /// Pop values from the value stack so that it is left at the
    /// input-parameters to an else-block.
    pub fn truncate_value_stack_to_else_params(&self, stack: &mut Vec<Value>) {
        debug_assert!(matches!(self, &ControlStackFrame::If { .. }));
        stack.truncate(self.original_stack_size());
    }

    /// Pop values from the value stack so that it is left at the state it was
    /// before this control-flow frame.
    pub fn truncate_value_stack_to_original_size(&self, stack: &mut Vec<Value>) {
        // The "If" frame pushes its parameters twice, so they're available to the else block
        // (see also `FuncTranslationState::push_if`).
        // Yet, the original_stack_size member accounts for them only once, so that the else
        // block can see the same number of parameters as the consequent block. As a matter of
        // fact, we need to subtract an extra number of parameter values for if blocks.
        let num_duplicated_params = match self {
            &ControlStackFrame::If {
                num_param_values, ..
            } => {
                debug_assert!(num_param_values <= self.original_stack_size());
                num_param_values
            }
            _ => 0,
        };
        stack.truncate(
            self.original_stack_size()
                .wrapping_sub(num_duplicated_params),
        );
    }
}

/// Information about the presence of an associated `else` for an `if`, or the
/// lack thereof.
#[derive(Debug)]
pub enum ElseData {
    /// The `if` does not already have an `else` block.
    ///
    /// This doesn't mean that it will never have an `else`, just that we
    /// haven't seen it yet.
    NoElse {
        /// If we discover that we need an `else` block, this is the jump
        /// instruction that needs to be fixed up to point to the new `else`
        /// block rather than the destination block after the `if...end`.
        branch_inst: Inst,

        /// The placeholder block we're replacing.
        placeholder: Block,
    },

    /// We have already allocated an `else` block.
    ///
    /// Usually we don't know whether we will hit an `if .. end` or an `if
    /// .. else .. end`, but sometimes we can tell based on the block's type
    /// signature that the signature is not valid if there isn't an `else`. In
    /// these cases, we pre-allocate the `else` block.
    WithElse {
        /// This is the `else` block.
        else_block: Block,
    },
}

fn num_wasm_parameters(signature: &ir::Signature, env: &TranslationEnvironment) -> usize {
    (0..signature.params.len())
        .filter(|index| env.is_wasm_parameter(*index))
        .count()
}
