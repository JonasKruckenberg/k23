// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::indices::{FuncIndex, GlobalIndex};
use crate::wasm_unsupported;
use smallvec::SmallVec;

/// A constant expression.
///
/// These are used to initialize globals, table elements, etc...
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ConstExpr {
    ops: SmallVec<[ConstOp; 2]>,
}

impl ConstExpr {
    /// Create a new const expression from a `wasmparser` const expression.
    ///
    /// Returns the new const expression as well as the escaping function
    /// indices that appeared in `ref.func` instructions, if any.
    pub fn from_wasmparser(
        expr: &wasmparser::ConstExpr<'_>,
    ) -> crate::wasm::Result<(Self, SmallVec<[FuncIndex; 1]>)> {
        let mut iter = expr
            .get_operators_reader()
            .into_iter_with_offsets()
            .peekable();

        let mut ops = SmallVec::<[ConstOp; 2]>::new();
        let mut escaped = SmallVec::<[FuncIndex; 1]>::new();
        while let Some(res) = iter.next() {
            let (op, offset) = res?;

            // If we reach an `end` instruction, and there are no more
            // instructions after that, then we are done reading this const
            // expression.
            if matches!(op, wasmparser::Operator::End) && iter.peek().is_none() {
                break;
            }

            // Track any functions that appear in `ref.func` so that callers can
            // make sure to flag them as escaping.
            if let wasmparser::Operator::RefFunc { function_index } = &op {
                escaped.push(FuncIndex::from_u32(*function_index));
            }

            ops.push(ConstOp::from_wasmparser(op, offset)?);
        }
        Ok((Self { ops }, escaped))
    }

    pub fn ops(&self) -> impl ExactSizeIterator<Item = ConstOp> + use<'_> {
        self.ops.iter().copied()
    }
}

/// The subset of Wasm opcodes that are constant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ConstOp {
    I32Const(i32),
    I64Const(i64),
    F32Const(u32),
    F64Const(u64),
    V128Const(u128),
    GlobalGet(GlobalIndex),
    RefI31,
    RefNull,
    RefFunc(FuncIndex),
    I32Add,
    I32Sub,
    I32Mul,
    I64Add,
    I64Sub,
    I64Mul,
}

impl ConstOp {
    /// Convert a `wasmparser::Operator` to a `ConstOp`.
    pub fn from_wasmparser(
        op: wasmparser::Operator<'_>,
        offset: usize,
    ) -> crate::wasm::Result<Self> {
        use wasmparser::Operator as O;
        Ok(match op {
            O::I32Const { value } => Self::I32Const(value),
            O::I64Const { value } => Self::I64Const(value),
            O::F32Const { value } => Self::F32Const(value.bits()),
            O::F64Const { value } => Self::F64Const(value.bits()),
            O::V128Const { value } => Self::V128Const(u128::from_le_bytes(*value.bytes())),
            O::RefNull { hty: _ } => Self::RefNull,
            O::RefFunc { function_index } => Self::RefFunc(FuncIndex::from_u32(function_index)),
            O::GlobalGet { global_index } => Self::GlobalGet(GlobalIndex::from_u32(global_index)),
            O::RefI31 => Self::RefI31,
            O::I32Add => Self::I32Add,
            O::I32Sub => Self::I32Sub,
            O::I32Mul => Self::I32Mul,
            O::I64Add => Self::I64Add,
            O::I64Sub => Self::I64Sub,
            O::I64Mul => Self::I64Mul,
            op => {
                return Err(wasm_unsupported!(
                    "unsupported opcode in const expression at offset {offset:#x}: {op:?}",
                ));
            }
        })
    }
}
