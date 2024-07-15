use crate::runtime::instance::InstanceData;
use crate::runtime::vmcontext::VMVal;
use alloc::vec::Vec;
use cranelift_wasm::{ConstExpr, ConstOp, GlobalIndex};

#[derive(Default)]
pub struct ConstExprEvaluator {
    stack: Vec<VMVal>,
}

impl ConstExprEvaluator {
    pub fn eval(&mut self, instance: &mut InstanceData, expr: &ConstExpr) -> VMVal {
        for op in expr.ops() {
            match op {
                ConstOp::I32Const(v) => self.push(VMVal { i32: *v }),
                ConstOp::I64Const(v) => self.push(VMVal { i64: *v }),
                ConstOp::F32Const(v) => self.push(VMVal { f32: *v }),
                ConstOp::F64Const(v) => self.push(VMVal { f64: *v }),
                ConstOp::V128Const(v) => self.push(VMVal {
                    v128: v.to_ne_bytes(),
                }),
                ConstOp::GlobalGet(global_index) => {
                    let val = self.global_get(instance, *global_index);
                    self.push(val);
                }
                _ => todo!(),
            }
        }

        assert_eq!(self.stack.len(), 1);
        self.stack.pop().unwrap()
    }

    fn push(&mut self, val: VMVal) {
        self.stack.push(val);
    }

    fn global_get(&mut self, instance: &mut InstanceData, index: GlobalIndex) -> VMVal {
        if let Some(def_index) = instance.module_info.module.defined_global_index(index) {
            let global_definition = unsafe { instance.global_ptr(def_index).as_ref().unwrap() };
            let ty = instance.module_info.module.globals[index];
            unsafe { global_definition.to_vmval(&ty.wasm_ty) }
        } else {
            todo!("imported globals")
            // self.imported_global(index).from
        }
    }
}
