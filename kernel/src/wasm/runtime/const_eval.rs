// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::runtime::vmcontext::VMVal;
use crate::wasm::translate::{ConstExpr, ConstOp};
use smallvec::SmallVec;

/// Simple interpreter for constant expressions.
#[derive(Debug, Default)]
pub struct ConstExprEvaluator {
    stack: SmallVec<[VMVal; 2]>,
}

impl ConstExprEvaluator {
    /// Evaluate a `ConstExpr` returning the result value.
    ///
    /// The only use of const expressions at the moment is to produce init values for globals,
    /// or tables or to calculate offsets. As such all uses *require* a const expression to return
    /// exactly one result.
    ///
    /// # Errors
    ///
    /// TODO
    ///
    /// # Panics
    ///
    /// The only uses of const expressions require them to evaluate to exactly one result.
    /// This method will panic if there is not exactly one result.
    pub fn eval(&mut self, expr: &ConstExpr) -> VMVal {
        for op in expr.ops() {
            match op {
                ConstOp::I32Const(value) => self.push(VMVal::i32(value)),
                ConstOp::I64Const(value) => self.push(VMVal::i64(value)),
                ConstOp::F32Const(value) => self.push(VMVal::f32(value)),
                ConstOp::F64Const(value) => self.push(VMVal::f64(value)),
                ConstOp::V128Const(value) => self.push(VMVal::v128(value)),
                ConstOp::GlobalGet(_) => todo!(),
                ConstOp::RefI31 => todo!(),
                ConstOp::RefNull => todo!(),
                ConstOp::RefFunc(_) => todo!(),
                ConstOp::I32Add => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i32(arg1.get_i32().wrapping_add(arg2.get_i32())));
                }
                ConstOp::I32Sub => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i32(arg1.get_i32().wrapping_sub(arg2.get_i32())));
                }
                ConstOp::I32Mul => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i32(arg1.get_i32().wrapping_mul(arg2.get_i32())));
                }
                ConstOp::I64Add => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i64(arg1.get_i64().wrapping_add(arg2.get_i64())));
                }
                ConstOp::I64Sub => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i64(arg1.get_i64().wrapping_sub(arg2.get_i64())));
                }
                ConstOp::I64Mul => {
                    let (arg1, arg2) = self.pop2();

                    self.push(VMVal::i64(arg1.get_i64().wrapping_mul(arg2.get_i64())));
                }
            }
        }

        assert_eq!(self.stack.len(), 1);
        self.stack.pop().expect("empty stack")
    }

    fn push(&mut self, val: VMVal) {
        self.stack.push(val);
    }

    fn pop2(&mut self) -> (VMVal, VMVal) {
        let v2 = self.stack.pop().unwrap();
        let v1 = self.stack.pop().unwrap();
        (v1, v2)
    }
}
