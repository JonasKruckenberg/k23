use core::pin::Pin;

use crate::func::Func;
use crate::store::StoreOpaque;
use crate::types::HeapTypeInner;
use crate::vm::VMVal;
use crate::{HeapType, ValType};

/// Possible runtime values that a WebAssembly module can either consume or
/// produce.
///
/// Note that we inline the `enum Ref { ... }` variants into `enum Val { ... }`
/// here as a size optimization.
#[derive(Debug, Clone, Copy)]
pub enum Val {
    /// A 32-bit integer.
    I32(i32),

    /// A 64-bit integer.
    I64(i64),

    /// A 32-bit float.
    ///
    /// Note that the raw bits of the float are stored here, and you can use
    /// `f32::from_bits` to create an `f32` value.
    F32(u32),

    /// A 64-bit float.
    ///
    /// Note that the raw bits of the float are stored here, and you can use
    /// `f64::from_bits` to create an `f64` value.
    F64(u64),

    // /// A 128-bit number.
    // V128(V128),
    /// A function reference.
    FuncRef(Option<Func>),
    // /// An external reference.
    // ExternRef(Option<Rooted<ExternRef>>),
    //
    // /// An internal reference.
    // AnyRef(Option<Rooted<AnyRef>>),
}

#[derive(Debug, Clone)]
pub enum Ref {
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
    // /// A reference to an value outside of the Wasm heap.
    // ///
    // /// These references are opaque to the Wasm itself. Wasm can't create
    // /// non-null external references, nor do anything with them accept pass them
    // /// around as function arguments and returns and place them into globals and
    // /// tables.
    // ///
    // /// Wasm can create null external references via the `ref.null extern`
    // /// instruction.
    // Extern(Option<Rooted<ExternRef>>),
    //
    // /// An internal reference.
    // ///
    // /// The `AnyRef` type represents WebAssembly `anyref` values. These can be
    // /// references to `struct`s and `array`s or inline/unboxed 31-bit
    // /// integers.
    // ///
    // /// Unlike `externref`, Wasm guests can directly allocate `anyref`s, and
    // /// does not need to rely on the host to do that.
    // Any(Option<Rooted<AnyRef>>),
}

// === impl Val ===

impl Val {
    /// Returns the null reference for the given heap type.
    #[inline]
    pub fn null_ref(heap_type: &HeapType) -> Val {
        Ref::null(heap_type).into()
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
            ValType::V128 => todo!(), // Some(Val::V128(V128::from(0))),
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
        self.load_ty(store)
    }

    pub(crate) fn ensure_matches_ty(
        &self,
        store: Pin<&mut StoreOpaque>,
        ty: &ValType,
    ) -> crate::Result<()> {
        todo!()
    }

    #[inline]
    #[expect(clippy::unnecessary_wraps, reason = "TODO")]
    pub(crate) fn load_ty(&self, store: &StoreOpaque) -> crate::Result<ValType> {
        Ok(match self {
            Val::I32(_) => ValType::I32,
            Val::I64(_) => ValType::I64,
            Val::F32(_) => ValType::F32,
            Val::F64(_) => ValType::F64,
            _ => todo!(),
            // Val::V128(_) => ValType::V128,
            // Val::ExternRef(Some(_)) => ValType::EXTERNREF,
            // Val::ExternRef(None) => ValType::NULLFUNCREF,
            // Val::FuncRef(None) => ValType::NULLFUNCREF,
            // Val::FuncRef(Some(f)) => ValType::Ref(RefType::new(
            //     false,
            //     HeapType::new(false, HeapTypeInner::ConcreteFunc(f.load_ty(store))),
            // )),
            // Val::AnyRef(None) => ValType::NULLREF,
            // Val::AnyRef(Some(a)) => ValType::Ref(RefType::new(false, a._ty(store)?)),
        })
    }

    pub(crate) fn from_vmval(_store: Pin<&mut StoreOpaque>, vmval: VMVal, ty: &ValType) -> Self {
        match ty {
            ValType::I32 => Self::I32(vmval.get_i32()),
            ValType::I64 => Self::I64(vmval.get_i64()),
            ValType::F32 => Self::F32(vmval.get_f32()),
            ValType::F64 => Self::F64(vmval.get_f64()),
            ValType::V128 => todo!(),
            ValType::Ref(_) => todo!(),
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

// === impl Ref ===

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

impl Ref {
    /// Create a null reference to the given heap type.
    #[inline]
    pub fn null(heap_type: &HeapType) -> Self {
        match heap_type.top().inner() {
            // HeapTypeInner::Any => Ref::Any(None),
            // HeapTypeInner::Extern => Ref::Extern(None),
            HeapTypeInner::Func => Ref::Func(None),
            ty => unreachable!("not a heap type: {ty:?}"),
        }
    }

    /// Is this a null reference?
    #[inline]
    pub fn is_null(&self) -> bool {
        match self {
            /*Ref::Any(None) | Ref::Extern(None) |*/ Ref::Func(None) => true,
            /*Ref::Any(Some(_)) | Ref::Extern(Some(_)) |*/ Ref::Func(Some(_)) => false,
        }
    }

    /// Is this a non-null reference?
    #[inline]
    pub fn is_non_null(&self) -> bool {
        !self.is_null()
    }
}
