use crate::error::ensure;
use crate::heap::Heap;
use crate::table::Table;
use crate::traits::FuncTranslationEnvironment;
use crate::Error;
use alloc::collections::btree_map::Entry;
use alloc::collections::BTreeMap;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::ir;
use cranelift_codegen::ir::immediates::Offset32;
use smallvec::{smallvec, SmallVec};

/// WASM to cranelift IR translation state
pub struct State {
    pub stack: SmallVec<ir::Value, 64>,
    pub control_stack: SmallVec<ControlFrame, 32>,
    pub reachable: bool,
    heaps: BTreeMap<wasmparser::MemIdx, Heap>,
    tables: BTreeMap<wasmparser::TableIdx, Table>,
    globals: BTreeMap<wasmparser::GlobalIdx, GlobalVariable>,
    functions: BTreeMap<wasmparser::FuncIdx, (ir::FuncRef, usize)>,
    signatures: BTreeMap<wasmparser::TypeIdx, (ir::SigRef, usize)>,
}

impl State {
    pub fn new() -> Self {
        Self {
            stack: smallvec![],
            control_stack: smallvec![],
            reachable: true,
            heaps: BTreeMap::new(),
            tables: BTreeMap::new(),
            globals: BTreeMap::new(),
            functions: BTreeMap::new(),
            signatures: BTreeMap::new(),
        }
    }

    /// Resets the state for another function compilation.
    pub fn reset(&mut self) {
        debug_assert!(self.stack.is_empty());
        debug_assert!(self.control_stack.is_empty());
        self.reachable = true;
    }

    pub fn pop1(&mut self) -> crate::Result<ir::Value> {
        ensure!(
            self.stack.len() >= 1,
            Error::EmptyStack {
                expected: 1,
                found: self.stack.len()
            }
        );

        Ok(self.stack.pop().unwrap())
    }

    pub fn pop2(&mut self) -> crate::Result<(ir::Value, ir::Value)> {
        ensure!(
            self.stack.len() >= 2,
            Error::EmptyStack {
                expected: 2,
                found: self.stack.len()
            }
        );

        let y = self.stack.pop().unwrap();
        let x = self.stack.pop().unwrap();

        Ok((x, y))
    }

    pub fn pop3(&mut self) -> crate::Result<(ir::Value, ir::Value, ir::Value)> {
        ensure!(
            self.stack.len() >= 3,
            Error::EmptyStack {
                expected: 3,
                found: self.stack.len()
            }
        );

        let z = self.stack.pop().unwrap();
        let y = self.stack.pop().unwrap();
        let x = self.stack.pop().unwrap();

        Ok((x, y, z))
    }

    pub fn popn(&mut self, n: usize) -> crate::Result<()> {
        ensure!(
            self.stack.len() >= n,
            Error::EmptyStack {
                expected: n,
                found: self.stack.len()
            }
        );
        self.stack.truncate(self.stack.len() - n);
        Ok(())
    }

    pub fn peek1(&self) -> crate::Result<ir::Value> {
        ensure!(
            self.stack.len() >= 1,
            Error::EmptyStack {
                expected: 1,
                found: self.stack.len()
            }
        );
        Ok(*self.stack.last().unwrap())
    }

    pub fn peekn(&self, n: usize) -> crate::Result<&[ir::Value]> {
        ensure!(
            self.stack.len() >= n,
            Error::EmptyStack {
                expected: n,
                found: self.stack.len()
            }
        );
        Ok(&self.stack[self.stack.len() - n..])
    }

    pub fn push1(&mut self, value: ir::Value) {
        self.stack.push(value);
    }

    pub fn pushn(&mut self, values: &[ir::Value]) {
        self.stack.extend_from_slice(values);
    }

    pub fn push_block(&mut self, next_block: ir::Block, num_params: usize, num_returns: usize) {
        debug_assert!(self.stack.len() >= num_params);
        self.control_stack.push(ControlFrame::Block {
            next_block,
            num_params,
            num_returns,
            original_stack_size: self.stack.len() - num_params,
            exit_is_branched_to: false,
        })
    }

    pub fn push_if(
        &mut self,
        next_block: ir::Block,
        else_state: ElseState,
        block_type: wasmparser::BlockType,
        num_params: usize,
        num_returns: usize,
    ) {
        debug_assert!(self.stack.len() >= num_params);

        self.stack.reserve(num_params);
        for i in (self.stack.len() - num_params)..self.stack.len() {
            let val = self.stack[i];
            self.stack.push(val);
        }

        self.control_stack.push(ControlFrame::If {
            next_block,
            num_params,
            num_returns,
            original_stack_size: self.stack.len() - num_params,

            is_consequent_start_reachable: self.reachable,
            is_consequent_end_reachable: None,
            exit_is_branched_to: false,

            else_state,
            block_type,
        })
    }

    pub fn push_loop(
        &mut self,
        body: ir::Block,
        next_block: ir::Block,
        num_params: usize,
        num_returns: usize,
    ) {
        self.control_stack.push(ControlFrame::Loop {
            next_block,
            body,
            num_params,
            num_returns,
            original_stack_size: self.stack.len() - num_params,
            exit_is_branched_to: false,
        })
    }

    pub fn truncate_value_stack_to_original_size(&mut self, frame: &ControlFrame) {
        // The "If" frame pushes its parameters twice, so they're available to the else block
        // (see also `FuncTranslationState::push_if`).
        // Yet, the original_stack_size member accounts for them only once, so that the else
        // block can see the same number of parameters as the consequent block. As a matter of
        // fact, we need to substract an extra number of parameter values for if blocks.
        let num_duplicated_params = match frame {
            &ControlFrame::If { num_params, .. } => {
                debug_assert!(num_params <= frame.original_stack_size());
                num_params
            }
            _ => 0,
        };
        self.stack
            .truncate(frame.original_stack_size() - num_duplicated_params);
    }

    pub fn get_or_make_table(
        &mut self,
        func: &mut ir::Function,
        table: wasmparser::TableIdx,
        env: &mut dyn FuncTranslationEnvironment,
    ) -> crate::Result<Table> {
        match self.tables.entry(table) {
            Entry::Occupied(entry) => Ok(*entry.get()),
            Entry::Vacant(entry) => Ok(*entry.insert(env.make_table(func, table)?)),
        }
    }

    pub fn get_or_make_heap(
        &mut self,
        func: &mut ir::Function,
        mem: wasmparser::MemIdx,
        env: &mut dyn FuncTranslationEnvironment,
    ) -> crate::Result<Heap> {
        match self.heaps.entry(mem) {
            Entry::Occupied(entry) => Ok(*entry.get()),
            Entry::Vacant(entry) => Ok(*entry.insert(env.make_heap(func, mem)?)),
        }
    }

    pub fn get_or_make_global(
        &mut self,
        func: FuncCursor,
        idx: wasmparser::GlobalIdx,
        env: &mut dyn FuncTranslationEnvironment,
    ) -> crate::Result<GlobalVariable> {
        match self.globals.entry(idx) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => Ok(entry.insert(env.make_global(func, idx)?).clone()),
        }
    }

    pub fn get_or_make_direct_func(
        &mut self,
        func: &mut ir::Function,
        index: wasmparser::FuncIdx,
        environ: &mut dyn FuncTranslationEnvironment,
    ) -> crate::Result<(ir::FuncRef, usize)> {
        match self.functions.entry(index) {
            Entry::Occupied(entry) => Ok(*entry.get()),
            Entry::Vacant(entry) => {
                let func_ref = environ.make_direct_func(func, index)?;
                let sig = func.dfg.ext_funcs[func_ref].signature;
                Ok(*entry.insert((func_ref, func.dfg.signatures[sig].params.len())))
            }
        }
    }

    pub fn get_or_make_indirect_sig(
        &mut self,
        func: &mut ir::Function,
        index: wasmparser::TypeIdx,
        environ: &mut dyn FuncTranslationEnvironment,
    ) -> crate::Result<(ir::SigRef, usize)> {
        match self.signatures.entry(index) {
            Entry::Occupied(entry) => Ok(*entry.get()),
            Entry::Vacant(entry) => {
                let sig_ref = environ.make_indirect_sig(func, index)?;
                Ok(*entry.insert((sig_ref, func.dfg.signatures[sig_ref].params.len())))
            }
        }
    }
}

#[derive(Clone)]
pub enum GlobalVariable {
    /// The global variable is a constant
    Const(ir::Value),
    /// The global variable is located in memory
    Memory {
        /// The base address of the global variable storage.
        gv: ir::GlobalValue,
        /// The offset of the global variable relative to the base address
        offset: Offset32,
        /// The type of the global
        ty: ir::Type,
    },
    /// The global variable is maintained by the host and has to be accessed through host calls
    Host,
}

#[derive(Debug)]
pub enum ControlFrame {
    Block {
        next_block: ir::Block,
        num_params: usize,
        num_returns: usize,
        original_stack_size: usize,
        exit_is_branched_to: bool,
    },
    Loop {
        next_block: ir::Block,
        body: ir::Block,
        num_params: usize,
        num_returns: usize,
        original_stack_size: usize,
        exit_is_branched_to: bool,
    },
    If {
        next_block: ir::Block,
        num_params: usize,
        num_returns: usize,
        original_stack_size: usize,
        // reachability
        is_consequent_start_reachable: bool,
        is_consequent_end_reachable: Option<bool>,
        exit_is_branched_to: bool,

        // data required for else blocks
        else_state: ElseState,
        block_type: wasmparser::BlockType,
    },
}

#[derive(Debug)]
pub enum ElseState {
    Absent {
        branch_inst: ir::Inst,
        placeholder: ir::Block,
    },
    Present {
        else_block: ir::Block,
    },
}

impl ControlFrame {
    pub fn num_params(&self) -> usize {
        match self {
            ControlFrame::Block { num_params, .. } => *num_params,
            ControlFrame::If { num_params, .. } => *num_params,
            ControlFrame::Loop { num_params, .. } => *num_params,
        }
    }

    pub fn num_returns(&self) -> usize {
        match self {
            ControlFrame::Block { num_returns, .. } => *num_returns,
            ControlFrame::If { num_returns, .. } => *num_returns,
            ControlFrame::Loop { num_returns, .. } => *num_returns,
        }
    }

    pub fn is_loop(&self) -> bool {
        matches!(self, ControlFrame::Loop { .. })
    }

    pub fn next_block(&self) -> ir::Block {
        match self {
            ControlFrame::Block { next_block, .. } => *next_block,
            ControlFrame::If { next_block, .. } => *next_block,
            ControlFrame::Loop { next_block, .. } => *next_block,
        }
    }

    pub fn br_destination(&self) -> ir::Block {
        match *self {
            Self::If { next_block, .. } | Self::Block { next_block, .. } => next_block,
            Self::Loop { body, .. } => body,
        }
    }

    pub fn original_stack_size(&self) -> usize {
        match self {
            ControlFrame::Block {
                original_stack_size,
                ..
            } => *original_stack_size,
            ControlFrame::If {
                original_stack_size,
                ..
            } => *original_stack_size,
            ControlFrame::Loop {
                original_stack_size,
                ..
            } => *original_stack_size,
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
}
