// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::store::StoreOpaque;
use crate::wasm::types::{HeapType, HeapTypeInner, RefType, ValType};
use crate::wasm::utils::enum_accessors;
use crate::wasm::vm::{TableElement, VMVal};
use crate::wasm::Func;
use anyhow::bail;
use core::ptr;

#[derive(Debug)]
pub enum Val {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
    V128(u128),
    /// A first-class reference to a WebAssembly function.
    ///
    /// The host, or the Wasm guest, can invoke this function.
    ///
    /// The host can create function references via [`Func::new`] or
    /// [`Func::wrap`].
    ///
    /// The Wasm guest can create non-null function references via the
    /// `ref.func` instruction, or null references via the `ref.null func`
    /// instruction.
    FuncRef(Option<Func>),
}

impl Val {
    /// Returns the null reference for the given heap type.
    #[inline]
    pub fn null_ref(heap_type: &HeapType) -> Val {
        Ref::null(&heap_type).into()
    }

    /// Returns the null function reference value.
    ///
    /// The return value has type `(ref null nofunc)` aka `nullfuncref` and is a
    /// subtype of all function references.
    #[inline]
    pub const fn null_func_ref() -> Val {
        Val::FuncRef(None)
    }

    // /// Returns the null function reference value.
    // ///
    // /// The return value has type `(ref null extern)` aka `nullexternref` and is
    // /// a subtype of all external references.
    // #[inline]
    // pub const fn null_extern_ref() -> Val {
    //     Val::ExternRef(None)
    // }
    //
    // /// Returns the null function reference value.
    // ///
    // /// The return value has type `(ref null any)` aka `nullref` and is a
    // /// subtype of all internal references.
    // #[inline]
    // pub const fn null_any_ref() -> Val {
    //     Val::AnyRef(None)
    // }

    /// Returns the default value for the given type, if any exists.
    ///
    /// Returns `None` if there is no default value for the given type (for
    /// example, non-nullable reference types do not have a default value).
    pub fn default_for_ty(ty: &ValType) -> Option<Val> {
        match ty {
            ValType::I32 => Some(Val::I32(0)),
            ValType::I64 => Some(Val::I64(0)),
            ValType::F32 => Some(Val::F32(0)),
            ValType::F64 => Some(Val::F64(0)),
            ValType::V128 => Some(Val::V128(0)),
            ValType::Ref(ref_ty) => {
                if ref_ty.is_nullable() {
                    Some(Val::null_ref(ref_ty.heap_type()))
                } else {
                    None
                }
            }
        }
    }

    /// Returns the corresponding [`ValType`] for this `Val`.
    ///
    /// # Errors
    ///
    /// Returns an error if this value is a GC reference that has since been
    /// unrooted.
    ///
    /// # Panics
    ///
    /// Panics if this value is associated with a different store.
    #[inline]
    pub fn ty(&self, store: &StoreOpaque) -> crate::Result<ValType> {
        Ok(match self {
            Val::I32(_) => ValType::I32,
            Val::I64(_) => ValType::I64,
            Val::F32(_) => ValType::F32,
            Val::F64(_) => ValType::F64,
            Val::V128(_) => ValType::V128,
            Val::FuncRef(None) => ValType::NULLFUNCREF,
            Val::FuncRef(Some(f)) => ValType::Ref(RefType::new(
                false,
                HeapType::concrete_func(f.ty(store)),
            )),
            // Val::ExternRef(Some(_)) => ValType::EXTERNREF,
            // Val::ExternRef(None) => ValType::NULLFUNCREF,
            // Val::AnyRef(None) => ValType::NULLREF,
            // Val::AnyRef(Some(a)) => ValType::Ref(RefType::new(false, a._ty(store)?)),
        })
    }

    /// Does this value match the given type?
    ///
    /// Returns an error is an underlying `Rooted` has been unrooted.
    ///
    /// # Panics
    ///
    /// Panics if this value is not associated with the given store.
    pub fn matches_ty(&self, store: &StoreOpaque, ty: &ValType) -> crate::Result<bool> {
        assert!(self.comes_from_same_store(store));
        assert!(ty.comes_from_same_engine(store.engine()));
        Ok(match (self, ty) {
            (Val::I32(_), ValType::I32)
            | (Val::I64(_), ValType::I64)
            | (Val::F32(_), ValType::F32)
            | (Val::F64(_), ValType::F64)
            | (Val::V128(_), ValType::V128) => true,

            (Val::FuncRef(f), ValType::Ref(ref_ty)) => Ref::from(*f).matches_ty(store, ref_ty)?,

            (Val::I32(_), _)
            | (Val::I64(_), _)
            | (Val::F32(_), _)
            | (Val::F64(_), _)
            | (Val::V128(_), _)
            | (Val::FuncRef(_), _) => false,
        })
    }

    pub(crate) fn ensure_matches_ty(&self, store: &StoreOpaque, ty: &ValType) -> crate::Result<()> {
        if !self.comes_from_same_store(store) {
            bail!("value used with wrong store")
        }
        if !ty.comes_from_same_engine(store.engine()) {
            bail!("type used with wrong engine")
        }
        if self.matches_ty(store, ty)? {
            Ok(())
        } else {
            let actual_ty = self.ty(store)?;
            bail!("type mismatch: expected {ty}, found {actual_ty}")
        }
    }

    /// Convenience method to convert this [`Val`] into a [`ValRaw`].
    ///
    /// Returns an error if this value is a GC reference and the GC reference
    /// has been unrooted.
    ///
    /// # Unsafety
    ///
    /// This method is unsafe for the reasons that [`ExternRef::to_raw`] and
    /// [`Func::to_raw`] are unsafe.
    pub(super) unsafe fn to_vmval(&self, store: &mut StoreOpaque) -> crate::Result<VMVal> {
        // Safety: ensured by caller
        unsafe {
            match self {
                Val::I32(i) => Ok(VMVal::i32(*i)),
                Val::I64(i) => Ok(VMVal::i64(*i)),
                Val::F32(u) => Ok(VMVal::f32(*u)),
                Val::F64(u) => Ok(VMVal::f64(*u)),
                Val::V128(b) => Ok(VMVal::v128(*b)),
                Val::FuncRef(f) => Ok(VMVal::funcref(match f {
                    None => ptr::null_mut(),
                    Some(e) => e.to_vmval(store),
                })),
            }
        }
    }

    /// Convenience method to convert a [`ValRaw`] into a [`Val`].
    ///
    /// # Unsafety
    ///
    /// This method is unsafe for the reasons that [`ExternRef::from_vmval`] and
    /// [`Func::from_vmval`] are unsafe. Additionally there's no guarantee
    /// otherwise that `raw` should have the type `ty` specified.
    pub(super) unsafe fn from_vmval(store: &mut StoreOpaque, vmval: VMVal, ty: ValType) -> Val {
        unsafe {
            match ty {
                ValType::I32 => Val::I32(vmval.get_i32()),
                ValType::I64 => Val::I64(vmval.get_i64()),
                ValType::F32 => Val::F32(vmval.get_f32()),
                ValType::F64 => Val::F64(vmval.get_f64()),
                ValType::V128 => Val::V128(vmval.get_v128().into()),
                ValType::Ref(ref_ty) => {
                    let ref_ = match ref_ty.heap_type().inner {
                        HeapTypeInner::Func | HeapTypeInner::ConcreteFunc(_) => {
                            Func::from_vmval(store, vmval.get_funcref()).into()
                        }

                        HeapTypeInner::NoFunc => Ref::Func(None),

                        HeapTypeInner::Extern => todo!(),

                        HeapTypeInner::NoExtern => todo!(),

                        HeapTypeInner::Any
                        | HeapTypeInner::Eq
                        | HeapTypeInner::I31
                        | HeapTypeInner::Array
                        | HeapTypeInner::ConcreteArray(_)
                        | HeapTypeInner::Struct
                        | HeapTypeInner::ConcreteStruct(_) => {
                            todo!()
                        }
                        HeapTypeInner::None => todo!(),

                        HeapTypeInner::Exn | HeapTypeInner::NoExn => todo!(),
                        HeapTypeInner::Cont | HeapTypeInner::NoCont => todo!(),
                    };
                    assert!(
                        ref_ty.is_nullable() || !ref_.is_null(),
                        "if the type is not nullable, we shouldn't get null; got \
                     type = {ref_ty}, ref = {ref_:?}"
                    );
                    ref_.into()
                }
            }
        }
    }

    enum_accessors! {
        e
        (I32(i32) i32 get_i32 unwrap_i32 *e)
        (I64(i64) i64 get_i64 unwrap_i64 *e)
        (F32(f32) f32 get_f32 unwrap_f32 f32::from_bits(*e))
        (F64(f64) f64 get_f64 unwrap_f64 f64::from_bits(*e))
        (V128(u128) v128 get_v128 unwrap_v128 *e)
        (FuncRef(Option<&Func>) func_ref get_func_ref unwrap_func_ref e.as_ref())
        // (ExternRef(Option<&Rooted<ExternRef>>) extern_ref unwrap_extern_ref e.as_ref())
        // (AnyRef(Option<&Rooted<AnyRef>>) any_ref unwrap_any_ref e.as_ref())
    }

    #[inline]
    pub(crate) fn comes_from_same_store(&self, store: &StoreOpaque) -> bool {
        match self {
            Val::FuncRef(Some(f)) => f.comes_from_same_store(store),
            Val::FuncRef(None) => true,

            // Val::ExternRef(Some(x)) => x.comes_from_same_store(store),
            // Val::ExternRef(None) => true,
            //
            // Val::AnyRef(Some(a)) => a.comes_from_same_store(store),
            // Val::AnyRef(None) => true,

            // Integers, floats, and vectors have no association with any
            // particular store, so they're always considered as "yes I came
            // from that store",
            Val::I32(_) | Val::I64(_) | Val::F32(_) | Val::F64(_) | Val::V128(_) => true,
        }
    }
}

impl From<i32> for Val {
    #[inline]
    fn from(val: i32) -> Val {
        Val::I32(val)
    }
}

impl From<i64> for Val {
    #[inline]
    fn from(val: i64) -> Val {
        Val::I64(val)
    }
}

impl From<f32> for Val {
    #[inline]
    fn from(val: f32) -> Val {
        Val::F32(val.to_bits())
    }
}

impl From<f64> for Val {
    #[inline]
    fn from(val: f64) -> Val {
        Val::F64(val.to_bits())
    }
}

impl From<Ref> for Val {
    #[inline]
    fn from(val: Ref) -> Val {
        match val {
            Ref::Func(f) => Val::FuncRef(f),
            // Ref::Extern(e) => Val::ExternRef(e),
            // Ref::Any(a) => Val::AnyRef(a),
        }
    }
}

impl From<Func> for Val {
    #[inline]
    fn from(val: Func) -> Val {
        Val::FuncRef(Some(val))
    }
}

impl From<Option<Func>> for Val {
    #[inline]
    fn from(val: Option<Func>) -> Val {
        Val::FuncRef(val)
    }
}

impl From<u128> for Val {
    #[inline]
    fn from(val: u128) -> Val {
        Val::V128(val.into())
    }
}

#[derive(Debug)]
pub enum Ref {
    // NB: We have a variant for each of the type hierarchies defined in Wasm,
    // and push the `Option` that provides nullability into each variant. This
    // allows us to get the most-precise type of any reference value, whether it
    // is null or not, without any additional metadata.
    //
    // Consider if we instead had the nullability inside `Val::Ref` and each of
    // the `Ref` variants did not have an `Option`:
    //
    //     enum Val {
    //         Ref(Option<Ref>),
    //         // Etc...
    //     }
    //     enum Ref {
    //         Func(Func),
    //         External(ExternRef),
    //         // Etc...
    //     }
    //
    // In this scenario, what type would we return from `Val::ty` for
    // `Val::Ref(None)`? Because Wasm has multiple separate type hierarchies,
    // there is no single common bottom type for all the different kinds of
    // references. So in this scenario, `Val::Ref(None)` doesn't have enough
    // information to reconstruct the value's type. That's a problem for us
    // because we need to get a value's type at various times all over the code
    // base.
    //
    /// A first-class reference to a WebAssembly function.
    ///
    /// The host, or the Wasm guest, can invoke this function.
    ///
    /// The host can create function references via [`Func::new`] or
    /// [`Func::wrap`].
    ///
    /// The Wasm guest can create non-null function references via the
    /// `ref.func` instruction, or null references via the `ref.null func`
    /// instruction.
    Func(Option<Func>),
}

impl Ref {
    /// Create a null reference to the given heap type.
    #[inline]
    pub fn null(heap_type: &HeapType) -> Self {
        match heap_type.top().inner {
            // HeapType::Any => Ref::Any(None),
            // HeapType::Extern => Ref::Extern(None),
            HeapTypeInner::Func => Ref::Func(None),
            ty => unreachable!("not a heap type: {ty:?}"),
        }
    }

    /// Is this a null reference?
    #[inline]
    pub fn is_null(&self) -> bool {
        match self {
            Ref::Func(None) => true,
            Ref::Func(Some(_)) => false,
            //
            // Ref::Any(None) | Ref::Extern(None) | Ref::Func(None) => true,
            // Ref::Any(Some(_)) | Ref::Extern(Some(_)) | Ref::Func(Some(_)) => false,
        }
    }

    /// Is this a non-null reference?
    #[inline]
    pub fn is_non_null(&self) -> bool {
        !self.is_null()
    }

    /// Get the type of this reference.
    ///
    /// # Errors
    ///
    /// Return an error if this reference has been unrooted.
    ///
    /// # Panics
    ///
    /// Panics if this reference is associated with a different store.
    pub fn ty(&self, store: &StoreOpaque) -> crate::Result<RefType> {
        assert!(self.comes_from_same_store(store));
        Ok(RefType::new(
            self.is_null(),
            // NB: We choose the most-specific heap type we can here and let
            // subtyping do its thing if callers are matching against a
            // `HeapType::Func`.
            match self {
                // Ref::Extern(None) => HeapType::NoExtern,
                // Ref::Extern(Some(_)) => HeapType::Extern,
                Ref::Func(None) => HeapType {
                    shared: false,
                    inner: HeapTypeInner::NoFunc,
                },
                Ref::Func(Some(f)) => HeapType {
                    shared: false,
                    inner: HeapTypeInner::ConcreteFunc(f.ty(store)),
                },
                // Ref::Any(None) => HeapType::None,
                // Ref::Any(Some(a)) => a._ty(store)?,
            },
        ))
    }

    pub fn matches_ty(&self, store: &StoreOpaque, ty: &RefType) -> crate::Result<bool> {
        assert!(self.comes_from_same_store(store));
        assert!(ty.comes_from_same_engine(store.engine()));
        if self.is_null() && !ty.is_nullable() {
            return Ok(false);
        }
        Ok(match (self, &ty.heap_type().inner) {
            // (Ref::Extern(_), HeapType::Extern) => true,
            // (Ref::Extern(None), HeapType::NoExtern) => true,
            // (Ref::Extern(_), _) => false,
            (Ref::Func(_), HeapTypeInner::Func) => true,
            (Ref::Func(None), HeapTypeInner::NoFunc | HeapTypeInner::ConcreteFunc(_)) => true,
            (Ref::Func(Some(f)), HeapTypeInner::ConcreteFunc(func_ty)) => {
                f.matches_ty(store, func_ty.clone())
            }
            (Ref::Func(_), _) => false,
            // (Ref::Any(_), HeapType::Any) => true,
            // (Ref::Any(Some(a)), HeapType::I31) => a._is_i31(store)?,
            // (Ref::Any(Some(a)), HeapType::Struct) => a._is_struct(store)?,
            // (Ref::Any(Some(a)), HeapType::ConcreteStruct(_ty)) => match a._as_struct(store)? {
            //     None => false,
            //     #[cfg_attr(not(feature = "gc"), allow(unreachable_patterns))]
            //     Some(s) => s._matches_ty(store, _ty)?,
            // },
            // (Ref::Any(Some(a)), HeapType::Eq) => a._is_eqref(store)?,
            // (Ref::Any(Some(a)), HeapType::Array) => a._is_array(store)?,
            // (Ref::Any(Some(a)), HeapType::ConcreteArray(_ty)) => match a._as_array(store)? {
            //     None => false,
            //     #[cfg_attr(not(feature = "gc"), allow(unreachable_patterns))]
            //     Some(a) => a._matches_ty(store, _ty)?,
            // },
            // (
            //     Ref::Any(None),
            //     HeapType::None
            //     | HeapType::I31
            //     | HeapType::ConcreteStruct(_)
            //     | HeapType::Struct
            //     | HeapType::ConcreteArray(_)
            //     | HeapType::Array
            //     | HeapType::Eq,
            // ) => true,
            // (Ref::Any(_), _) => false,
        })
    }

    pub fn ensure_matches_ty(&self, store: &StoreOpaque, ty: &RefType) -> crate::Result<()> {
        if !self.comes_from_same_store(store) {
            bail!("reference used with wrong store")
        }
        if !ty.comes_from_same_engine(store.engine()) {
            bail!("type used with wrong engine")
        }
        if self.matches_ty(store, ty)? {
            Ok(())
        } else {
            let actual_ty = self.ty(store)?;
            bail!("type mismatch: expected {ty}, found {actual_ty}")
        }
    }

    pub(super) fn comes_from_same_store(&self, store: &StoreOpaque) -> bool {
        match self {
            Ref::Func(Some(f)) => f.comes_from_same_store(store),
            Ref::Func(None) => true,
            // Ref::Extern(Some(x)) => x.comes_from_same_store(store),
            // Ref::Extern(None) => true,
            // Ref::Any(Some(a)) => a.comes_from_same_store(store),
            // Ref::Any(None) => true,
        }
    }

    pub(crate) fn into_table_element(
        self,
        store: &mut StoreOpaque,
        ty: &RefType,
    ) -> crate::Result<TableElement> {
        let heap_top_ty = ty.heap_type().top();
        match (self, heap_top_ty) {
            (Ref::Func(None), HeapType { inner: HeapTypeInner::NoFunc, shared: _ }) => {
                assert!(ty.is_nullable());
                Ok(TableElement::FuncRef(None))
            }
            (Ref::Func(Some(f)), HeapType { inner: HeapTypeInner::Func, shared: _ }) => {
                debug_assert!(
                    f.comes_from_same_store(&store),
                    "checked in `ensure_matches_ty`"
                );
                Ok(TableElement::FuncRef(Some(f.vm_func_ref(store))))
            }
            _ => unimplemented!()
        }
    }

}

#[expect(irrefutable_let_patterns, reason = "there is only one variant rn")]
impl Ref {
    enum_accessors! {
        e
        (Func(Option<&Func>) func_ref get_func_ref unwrap_func_ref e.as_ref())
    }
}

impl From<Func> for Ref {
    #[inline]
    fn from(f: Func) -> Ref {
        Ref::Func(Some(f))
    }
}

impl From<Option<Func>> for Ref {
    #[inline]
    fn from(f: Option<Func>) -> Ref {
        Ref::Func(f)
    }
}
