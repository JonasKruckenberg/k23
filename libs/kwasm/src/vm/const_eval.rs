use alloc::vec::Vec;
use core::pin::Pin;

use anyhow::{Context, bail};
use cfg_if::cfg_if;
use smallvec::SmallVec;
use wasmparser::WasmFeatures;

use crate::StructType;
use crate::indices::{FuncIndex, GlobalIndex, VMSharedTypeIndex};
use crate::store::StoreOpaque;
use crate::utils::wasm_unsupported;
use crate::values::Val;
use crate::vm::instance::Instance;
use crate::vm::vmcontext::VMVal;
use crate::wasm::{
    ConstExpr, ConstOp, WasmCompositeType, WasmCompositeTypeInner, WasmStorageType, WasmSubType,
    WasmValType,
};

/// Simple interpreter for constant expressions.
#[derive(Debug, Default)]
pub struct ConstExprEvaluator {
    stack: SmallVec<[VMVal; 2]>,
}

/// The context within which a particular const expression is evaluated.
pub struct ConstEvalContext<'a> {
    pub(crate) instance: &'a mut Instance,
}

impl<'a> ConstEvalContext<'a> {
    /// Create a new context.
    ///
    /// # Safety
    ///
    /// The caller must ensure `Instance` has its vmctx correctly initialized
    pub unsafe fn new(instance: &'a mut Instance) -> Self {
        Self { instance }
    }

    fn global_get(
        &mut self,
        store: Pin<&mut StoreOpaque>,
        index: GlobalIndex,
    ) -> crate::Result<VMVal> {
        // Safety: the caller promised that the vmctx is correctly initialized
        unsafe {
            let global = self.instance.defined_or_imported_global(index).as_ref();
            global.to_vmval(
                store,
                self.instance.translated_module().globals[index].content_type,
            )
        }
    }

    fn ref_func(&mut self, index: FuncIndex) -> VMVal {
        VMVal::funcref(self.instance.get_func_ref(index).unwrap().as_ptr().cast())
    }

    fn struct_fields_len(
        &self,
        store: Pin<&mut StoreOpaque>,
        shared_ty: VMSharedTypeIndex,
    ) -> usize {
        let struct_ty = StructType::from_shared_type_index(store.engine(), shared_ty);
        let fields = struct_ty.fields();
        fields.len()
    }

    /// Safety: field values must be of the correct types.
    unsafe fn struct_new(
        &mut self,
        mut store: Pin<&mut StoreOpaque>,
        shared_ty: VMSharedTypeIndex,
        fields: &[VMVal],
    ) -> crate::Result<VMVal> {
        let struct_ty = StructType::from_shared_type_index(store.engine(), shared_ty);
        let _fields = fields
            .iter()
            .zip(struct_ty.fields())
            .map(|(raw, ty)| {
                let ty = ty.element_type().unpack();
                Val::from_vmval(store.as_mut(), *raw, ty)
            })
            .collect::<Vec<_>>();

        // let allocator = StructRefPre::_new(store, struct_ty);
        // let struct_ref = StructRef::_new(store, &allocator, &fields)?;
        // let raw = struct_ref.to_anyref()._to_raw(store)?;
        // Ok(VMVal::anyref(raw))

        todo!()
    }

    fn struct_new_default(
        &mut self,
        store: Pin<&mut StoreOpaque>,
        shared_ty: VMSharedTypeIndex,
    ) -> crate::Result<VMVal> {
        let module = self.instance.module();

        let borrowed = module
            .engine()
            .type_registry()
            .borrow(shared_ty)
            .expect("should have a registered type for struct");
        let WasmSubType {
            composite_type:
                WasmCompositeType {
                    shared: false,
                    inner: WasmCompositeTypeInner::Struct(struct_ty),
                },
            ..
        } = &*borrowed
        else {
            unreachable!("registered type should be a struct");
        };

        let fields = struct_ty
            .fields
            .iter()
            .map(|ty| {
                if let WasmStorageType::Val(WasmValType::Ref(r)) = ty.element_type {
                    assert!(r.nullable);
                }
                VMVal::null()
            })
            .collect::<SmallVec<[_; 8]>>();

        // Safety: TODO
        unsafe { self.struct_new(store, shared_ty, &fields) }
    }
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
    pub fn eval(
        &mut self,
        mut store: Pin<&mut StoreOpaque>,
        ctx: &mut ConstEvalContext,
        expr: &ConstExpr,
    ) -> crate::Result<VMVal> {
        for op in expr.ops() {
            match op {
                ConstOp::I32Const(value) => self.push(VMVal::i32(value)),
                ConstOp::I64Const(value) => self.push(VMVal::i64(value)),
                ConstOp::F32Const(value) => self.push(VMVal::f32(value)),
                ConstOp::F64Const(value) => self.push(VMVal::f64(value)),
                ConstOp::V128Const(_value) => {
                    cfg_if! {
                        if # [cfg(feature = "simd")] {
                            self.push(VMVal::v128(_value)),
                        } else {
                            wasm_unsupported!(WasmFeatures::SIMD, "enable `simd` feature")
                        }
                    }
                }
                ConstOp::GlobalGet(g) => self.stack.push(ctx.global_get(store.as_mut(), g)?),
                ConstOp::RefNull => self.stack.push(VMVal::null()),
                ConstOp::RefFunc(f) => self.stack.push(ctx.ref_func(f)),
                ConstOp::RefI31 => {
                    let i = self.pop()?.get_i32();
                    // let i31 = I31::wrapping_i32(i);
                    // let raw = VMGcRef::from_i31(i31).as_raw_u32();
                    // self.stack.push(VMVal::anyref(raw));

                    todo!()
                }
                ConstOp::I32Add => {
                    let (arg1, arg2) = self.pop2()?;

                    self.push(VMVal::i32(arg1.get_i32().wrapping_add(arg2.get_i32())));
                }
                ConstOp::I32Sub => {
                    let (arg1, arg2) = self.pop2()?;

                    self.push(VMVal::i32(arg1.get_i32().wrapping_sub(arg2.get_i32())));
                }
                ConstOp::I32Mul => {
                    let (arg1, arg2) = self.pop2()?;

                    self.push(VMVal::i32(arg1.get_i32().wrapping_mul(arg2.get_i32())));
                }
                ConstOp::I64Add => {
                    let (arg1, arg2) = self.pop2()?;

                    self.push(VMVal::i64(arg1.get_i64().wrapping_add(arg2.get_i64())));
                }
                ConstOp::I64Sub => {
                    let (arg1, arg2) = self.pop2()?;

                    self.push(VMVal::i64(arg1.get_i64().wrapping_sub(arg2.get_i64())));
                }
                ConstOp::I64Mul => {
                    let (arg1, arg2) = self.pop2()?;

                    self.push(VMVal::i64(arg1.get_i64().wrapping_mul(arg2.get_i64())));
                }
                ConstOp::StructNew {
                    struct_type_index: _,
                } => {
                    // let interned_type_index = ctx.instance.env_module().types[*struct_type_index]
                    //     .unwrap_engine_type_index();
                    // let len = ctx.struct_fields_len(&mut store, interned_type_index);
                    //
                    // if self.stack.len() < len {
                    //     bail!(
                    //         "const expr evaluation error: expected at least {len} values on the stack, found {}",
                    //         self.stack.len()
                    //     )
                    // }
                    //
                    // let start = self.stack.len() - len;
                    // let s = unsafe {
                    //     ctx.struct_new(&mut store, interned_type_index, &self.stack[start..])?
                    // };
                    // self.stack.truncate(start);
                    // self.stack.push(s);

                    todo!()
                }
                ConstOp::StructNewDefault {
                    struct_type_index: _,
                } => {
                    // let ty = ctx.instance.env_module().types[*struct_type_index]
                    //     .unwrap_engine_type_index();
                    // self.stack.push(ctx.struct_new_default(&mut store, ty)?);

                    todo!()
                }
                ConstOp::ArrayNew {
                    array_type_index: _,
                } => {
                    // let ty = ctx.instance.env_module().types[*array_type_index]
                    //     .unwrap_engine_type_index();
                    // let ty = ArrayType::from_shared_type_index(store.engine(), ty);
                    //
                    // #[allow(clippy::cast_sign_loss)]
                    // let len = self.pop()?.get_i32() as u32;
                    //
                    // let elem = Val::from_vmval(&mut store, self.pop()?, ty.element_type().unpack());
                    //
                    // let pre = ArrayRefPre::_new(&mut store, ty);
                    // let array = ArrayRef::_new(&mut store, &pre, &elem, len)?;
                    //
                    // self.stack
                    //     .push(VMVal::anyref(array.to_anyref()._to_raw(&mut store)?));

                    todo!()
                }
                ConstOp::ArrayNewDefault {
                    array_type_index: _,
                } => {
                    // let ty = ctx.instance.env_module().types[*array_type_index]
                    //     .unwrap_engine_type_index();
                    // let ty = ArrayType::from_shared_type_index(store.engine(), ty);
                    //
                    // #[allow(clippy::cast_sign_loss)]
                    // let len = self.pop()?.get_i32() as u32;
                    //
                    // let elem = Val::default_for_ty(ty.element_type().unpack())
                    //     .expect("type should have a default value");
                    //
                    // let pre = ArrayRefPre::_new(&mut store, ty);
                    // let array = ArrayRef::_new(&mut store, &pre, &elem, len)?;
                    //
                    // self.stack
                    //     .push(VMVal::anyref(array.to_anyref()._to_raw(&mut store)?));

                    todo!()
                }
                ConstOp::ArrayNewFixed {
                    array_type_index: _,
                    array_size: _,
                } => {
                    // let ty = ctx.instance.env_module().types[*array_type_index]
                    //     .unwrap_engine_type_index();
                    // let ty = ArrayType::from_shared_type_index(store.engine(), ty);
                    //
                    // let array_size = usize::try_from(*array_size).unwrap();
                    // if self.stack.len() < array_size {
                    //     bail!(
                    //         "const expr evaluation error: expected at least {array_size} values on the stack, found {}",
                    //         self.stack.len()
                    //     )
                    // }
                    //
                    // let start = self.stack.len() - array_size;
                    //
                    // let elem_ty = ty.element_type();
                    // let elem_ty = elem_ty.unpack();
                    //
                    // let elems = self
                    //     .stack
                    //     .drain(start..)
                    //     .map(|raw| Val::_from_raw(&mut store, raw, elem_ty))
                    //     .collect::<SmallVec<[_; 8]>>();
                    //
                    // let pre = ArrayRefPre::_new(&mut store, ty);
                    // let array = ArrayRef::_new_fixed(&mut store, &pre, &elems)?;
                    //
                    // self.stack
                    //     .push(VMVal::anyref(array.to_anyref()._to_raw(&mut store)?));

                    todo!()
                }
            }
        }

        if self.stack.len() == 1 {
            tracing::trace!("const expr evaluated to {:?}", self.stack[0]);
            Ok(self.stack.pop().unwrap())
        } else {
            let len = self.stack.len();
            // we need to correctly clear the stack here for the next time we try to use the const eval
            self.stack.clear();
            bail!("const expr evaluation error: expected 1 resulting value, found {len}",)
        }
    }

    fn push(&mut self, val: VMVal) {
        self.stack.push(val);
    }

    fn pop(&mut self) -> crate::Result<VMVal> {
        self.stack.pop().context("pop from empty stack")
    }

    fn pop2(&mut self) -> crate::Result<(VMVal, VMVal)> {
        let v2 = self.pop()?;
        let v1 = self.pop()?;
        Ok((v1, v2))
    }
}
