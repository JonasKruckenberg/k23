// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::indices::{CanonicalizedTypeIndex, VMSharedTypeIndex};
use crate::wasm::translate::{
    EntityType, Memory, ModuleTypes, Table, WasmHeapType, WasmHeapTypeInner, WasmRefType,
    WasmValType,
};
use crate::wasm::type_registry::RegisteredType;
use crate::wasm::Engine;
use anyhow::bail;
use core::fmt;
use core::fmt::Display;

/// Indicator of whether a global value, struct's field, or array type's
/// elements are mutable or not.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum Mutability {
    /// The global value, struct field, or array elements are constant and the
    /// value does not change.
    Const,
    /// The value of the global, struct field, or array elements can change over
    /// time.
    Var,
}

impl Mutability {
    /// Is this constant?
    #[inline]
    pub fn is_const(&self) -> bool {
        *self == Self::Const
    }

    /// Is this variable?
    #[inline]
    pub fn is_var(&self) -> bool {
        *self == Self::Var
    }
}

/// Indicator of whether a type is final or not.
///
/// Final types may not be the supertype of other types.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum Finality {
    /// The associated type is final.
    Final,
    /// The associated type is not final.
    NonFinal,
}

impl Finality {
    /// Is this final?
    #[inline]
    pub fn is_final(&self) -> bool {
        *self == Self::Final
    }

    /// Is this non-final?
    #[inline]
    pub fn is_non_final(&self) -> bool {
        *self == Self::NonFinal
    }
}

/// Possible value types in WebAssembly.
///
/// # Subtyping and Equality
///
/// `ValType` does not implement `Eq`, because reference types have a subtyping
/// relationship, and so 99.99% of the time you actually want to check whether
/// one type matches (i.e. is a subtype of) another type. You can use the
/// [`ValType::matches`] and [`Val::matches_ty`][crate::Val::matches_ty] methods
/// to perform these types of checks. If, however, you are in that 0.01%
/// scenario where you need to check precise equality between types, you can use
/// the [`ValType::eq`] method.
#[derive(Clone, Hash)]
pub enum ValType {
    // NB: the ordering of variants here is intended to match the ordering in
    // `wasmtime_environ::WasmType` to help improve codegen when converting.
    //
    /// Signed 32 bit integer.
    I32,
    /// Signed 64 bit integer.
    I64,
    /// Floating point 32 bit integer.
    F32,
    /// Floating point 64 bit integer.
    F64,
    /// A 128 bit number.
    V128,
    /// An opaque reference to some type on the heap.
    Ref(RefType),
}

impl fmt::Debug for ValType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Display for ValType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ValType::I32 => write!(f, "i32"),
            ValType::I64 => write!(f, "i64"),
            ValType::F32 => write!(f, "f32"),
            ValType::F64 => write!(f, "f64"),
            ValType::V128 => write!(f, "v128"),
            ValType::Ref(r) => Display::fmt(r, f),
        }
    }
}

impl ValType {
    /// The `externref` type, aka `(ref null extern)`.
    pub const EXTERNREF: Self = ValType::Ref(RefType::EXTERNREF);

    /// The `nullexternref` type, aka `(ref null noextern)`.
    pub const NULLEXTERNREF: Self = ValType::Ref(RefType::NULLEXTERNREF);

    /// The `funcref` type, aka `(ref null func)`.
    pub const FUNCREF: Self = ValType::Ref(RefType::FUNCREF);

    /// The `nullfuncref` type, aka `(ref null nofunc)`.
    pub const NULLFUNCREF: Self = ValType::Ref(RefType::NULLFUNCREF);

    /// The `anyref` type, aka `(ref null any)`.
    pub const ANYREF: Self = ValType::Ref(RefType::ANYREF);

    /// The `eqref` type, aka `(ref null eq)`.
    pub const EQREF: Self = ValType::Ref(RefType::EQREF);

    /// The `i31ref` type, aka `(ref null i31)`.
    pub const I31REF: Self = ValType::Ref(RefType::I31REF);

    /// The `arrayref` type, aka `(ref null array)`.
    pub const ARRAYREF: Self = ValType::Ref(RefType::ARRAYREF);

    /// The `structref` type, aka `(ref null struct)`.
    pub const STRUCTREF: Self = ValType::Ref(RefType::STRUCTREF);

    /// The `nullref` type, aka `(ref null none)`.
    pub const NULLREF: Self = ValType::Ref(RefType::NULLREF);

    /// Returns true if `ValType` matches any of the numeric types. (e.g. `I32`,
    /// `I64`, `F32`, `F64`).
    #[inline]
    pub fn is_num(&self) -> bool {
        match self {
            ValType::I32 | ValType::I64 | ValType::F32 | ValType::F64 => true,
            _ => false,
        }
    }

    /// Is this the `i32` type?
    #[inline]
    pub fn is_i32(&self) -> bool {
        matches!(self, ValType::I32)
    }

    /// Is this the `i64` type?
    #[inline]
    pub fn is_i64(&self) -> bool {
        matches!(self, ValType::I64)
    }

    /// Is this the `f32` type?
    #[inline]
    pub fn is_f32(&self) -> bool {
        matches!(self, ValType::F32)
    }

    /// Is this the `f64` type?
    #[inline]
    pub fn is_f64(&self) -> bool {
        matches!(self, ValType::F64)
    }

    /// Is this the `v128` type?
    #[inline]
    pub fn is_v128(&self) -> bool {
        matches!(self, ValType::V128)
    }

    /// Returns true if `ValType` is any kind of reference type.
    #[inline]
    pub fn is_ref(&self) -> bool {
        matches!(self, ValType::Ref(_))
    }

    /// Is this the `funcref` (aka `(ref null func)`) type?
    #[inline]
    pub fn is_funcref(&self) -> bool {
        matches!(
            self,
            ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType {
                    inner: HeapTypeInner::Func,
                    shared: _
                }
            })
        )
    }

    /// Is this the `externref` (aka `(ref null extern)`) type?
    #[inline]
    pub fn is_externref(&self) -> bool {
        matches!(
            self,
            ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType {
                    inner: HeapTypeInner::Extern,
                    shared: _
                }
            })
        )
    }

    /// Is this the `anyref` (aka `(ref null any)`) type?
    #[inline]
    pub fn is_anyref(&self) -> bool {
        matches!(
            self,
            ValType::Ref(RefType {
                nullable: true,
                heap_type: HeapType {
                    inner: HeapTypeInner::Any,
                    shared: _
                }
            })
        )
    }

    /// Get the underlying reference type, if this value type is a reference
    /// type.
    #[inline]
    pub fn as_ref(&self) -> Option<&RefType> {
        match self {
            ValType::Ref(r) => Some(r),
            _ => None,
        }
    }

    /// Get the underlying reference type, panicking if this value type is not a
    /// reference type.
    #[inline]
    pub fn unwrap_ref(&self) -> &RefType {
        self.as_ref()
            .expect("ValType::unwrap_ref on a non-reference type")
    }

    /// Does this value type match the other type?
    ///
    /// That is, is this value type a subtype of the other?
    ///
    /// # Panics
    ///
    /// Panics if either type is associated with a different engine from the
    /// other.
    pub fn matches(&self, other: &ValType) -> bool {
        match (self, other) {
            (Self::I32, Self::I32) => true,
            (Self::I64, Self::I64) => true,
            (Self::F32, Self::F32) => true,
            (Self::F64, Self::F64) => true,
            (Self::V128, Self::V128) => true,
            (Self::Ref(a), Self::Ref(b)) => a.matches(b),
            (Self::I32, _)
            | (Self::I64, _)
            | (Self::F32, _)
            | (Self::F64, _)
            | (Self::V128, _)
            | (Self::Ref(_), _) => false,
        }
    }

    /// Is value type `a` precisely equal to value type `b`?
    ///
    /// Returns `false` even if `a` is a subtype of `b` or vice versa, if they
    /// are not exactly the same value type.
    ///
    /// # Panics
    ///
    /// Panics if either type is associated with a different engine.
    pub fn eq(a: &Self, b: &Self) -> bool {
        a.matches(b) && b.matches(a)
    }

    pub fn ensure_matches(&self, engine: &Engine, other: &ValType) -> crate::Result<()> {
        if !self.comes_from_same_engine(engine) || !other.comes_from_same_engine(engine) {
            bail!("type used with wrong engine");
        }
        if self.matches(other) {
            Ok(())
        } else {
            bail!("type mismatch: expected {other}, found {self}")
        }
    }

    pub(crate) fn comes_from_same_engine(&self, engine: &Engine) -> bool {
        match self {
            Self::I32 | Self::I64 | Self::F32 | Self::F64 | Self::V128 => true,
            Self::Ref(r) => r.comes_from_same_engine(engine),
        }
    }

    pub(crate) fn to_wasm_type(&self) -> WasmValType {
        match self {
            Self::I32 => WasmValType::I32,
            Self::I64 => WasmValType::I64,
            Self::F32 => WasmValType::F32,
            Self::F64 => WasmValType::F64,
            Self::V128 => WasmValType::V128,
            Self::Ref(r) => WasmValType::Ref(r.to_wasm_type()),
        }
    }

    #[inline]
    pub(crate) fn from_wasm_type(engine: &Engine, ty: &WasmValType) -> Self {
        match ty {
            WasmValType::I32 => Self::I32,
            WasmValType::I64 => Self::I64,
            WasmValType::F32 => Self::F32,
            WasmValType::F64 => Self::F64,
            WasmValType::V128 => Self::V128,
            WasmValType::Ref(r) => Self::Ref(RefType::from_wasm_type(engine, r)),
        }
    }
}

/// Opaque references to data in the Wasm heap or to host data.
///
/// # Subtyping and Equality
///
/// `RefType` does not implement `Eq`, because reference types have a subtyping
/// relationship, and so 99.99% of the time you actually want to check whether
/// one type matches (i.e. is a subtype of) another type. You can use the
/// [`RefType::matches`] and [`Ref::matches_ty`][crate::Ref::matches_ty] methods
/// to perform these types of checks. If, however, you are in that 0.01%
/// scenario where you need to check precise equality between types, you can use
/// the [`RefType::eq`] method.
#[derive(Clone, Hash)]
pub struct RefType {
    nullable: bool,
    heap_type: HeapType,
}

impl fmt::Debug for RefType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl fmt::Display for RefType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(ref ")?;
        if self.is_nullable() {
            write!(f, "null ")?;
        }
        write!(f, "{})", self.heap_type())
    }
}

impl RefType {
    /// The `externref` type, aka `(ref null extern)`.
    pub const EXTERNREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::EXTERN,
    };

    /// The `nullexternref` type, aka `(ref null noextern)`.
    pub const NULLEXTERNREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::NOEXTERN,
    };

    /// The `funcref` type, aka `(ref null func)`.
    pub const FUNCREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::FUNC,
    };

    /// The `nullfuncref` type, aka `(ref null nofunc)`.
    pub const NULLFUNCREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::NOFUNC,
    };

    /// The `anyref` type, aka `(ref null any)`.
    pub const ANYREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::ANY,
    };

    /// The `eqref` type, aka `(ref null eq)`.
    pub const EQREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::EQ,
    };

    /// The `i31ref` type, aka `(ref null i31)`.
    pub const I31REF: Self = RefType {
        nullable: true,
        heap_type: HeapType::I31,
    };

    /// The `arrayref` type, aka `(ref null array)`.
    pub const ARRAYREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::ARRAY,
    };

    /// The `structref` type, aka `(ref null struct)`.
    pub const STRUCTREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::STRUCT,
    };

    /// The `nullref` type, aka `(ref null none)`.
    pub const NULLREF: Self = RefType {
        nullable: true,
        heap_type: HeapType::NONE,
    };

    /// Construct a new reference type.
    pub fn new(nullable: bool, heap_type: HeapType) -> RefType {
        RefType {
            nullable,
            heap_type,
        }
    }

    /// Can this type of reference be null?
    pub fn is_nullable(&self) -> bool {
        self.nullable
    }

    /// The heap type that this is a reference to.
    #[inline]
    pub fn heap_type(&self) -> &HeapType {
        &self.heap_type
    }

    /// Does this reference type match the other?
    ///
    /// That is, is this reference type a subtype of the other?
    ///
    /// # Panics
    ///
    /// Panics if either type is associated with a different engine from the
    /// other.
    pub fn matches(&self, other: &RefType) -> bool {
        if self.is_nullable() && !other.is_nullable() {
            return false;
        }
        self.heap_type().matches(other.heap_type())
    }

    /// Is reference type `a` precisely equal to reference type `b`?
    ///
    /// Returns `false` even if `a` is a subtype of `b` or vice versa, if they
    /// are not exactly the same reference type.
    ///
    /// # Panics
    ///
    /// Panics if either type is associated with a different engine.
    pub fn eq(a: &RefType, b: &RefType) -> bool {
        a.matches(b) && b.matches(a)
    }

    pub fn ensure_matches(&self, engine: &Engine, other: &RefType) -> crate::Result<()> {
        if !self.comes_from_same_engine(engine) || !other.comes_from_same_engine(engine) {
            bail!("type used with wrong engine");
        }
        if self.matches(other) {
            Ok(())
        } else {
            bail!("type mismatch: expected {other}, found {self}")
        }
    }

    pub(super) fn comes_from_same_engine(&self, engine: &Engine) -> bool {
        self.heap_type().comes_from_same_engine(engine)
    }

    pub(super) fn to_wasm_type(&self) -> WasmRefType {
        WasmRefType {
            nullable: self.is_nullable(),
            heap_type: self.heap_type().to_wasm_type(),
        }
    }

    pub(super) fn from_wasm_type(engine: &Engine, ty: &WasmRefType) -> RefType {
        RefType {
            nullable: ty.nullable,
            heap_type: HeapType::from_wasm_type(engine, &ty.heap_type),
        }
    }
}

#[derive(Debug, Clone, Hash)]
pub struct HeapType {
    pub shared: bool,
    pub inner: HeapTypeInner,
}

#[derive(Debug, Clone, Hash)]
pub enum HeapTypeInner {
    /// The abstract `extern` heap type represents external host data.
    ///
    /// This is the top type for the external type hierarchy, and therefore is
    /// the common supertype of all external reference types.
    Extern,
    /// The abstract `noextern` heap type represents the null external
    /// reference.
    ///
    /// This is the bottom type for the external type hierarchy, and therefore
    /// is the common subtype of all external reference types.
    NoExtern,

    /// The abstract `func` heap type represents a reference to any kind of
    /// function.
    ///
    /// This is the top type for the function references type hierarchy, and is
    /// therefore a supertype of every function reference.
    Func,
    /// A reference to a function of a specific, concrete type.
    ///
    /// These are subtypes of `func` and supertypes of `nofunc`.
    ConcreteFunc(FuncType),
    /// The abstract `nofunc` heap type represents the null function reference.
    ///
    /// This is the bottom type for the function references type hierarchy, and
    /// therefore `nofunc` is a subtype of all function reference types.
    NoFunc,

    /// The abstract `any` heap type represents all internal Wasm data.
    ///
    /// This is the top type of the internal type hierarchy, and is therefore a
    /// supertype of all internal types (such as `eq`, `i31`, `struct`s, and
    /// `array`s).
    Any,
    /// The abstract `eq` heap type represenets all internal Wasm references
    /// that can be compared for equality.
    ///
    /// This is a subtype of `any` and a supertype of `i31`, `array`, `struct`,
    /// and `none` heap types.
    Eq,
    /// The `i31` heap type represents unboxed 31-bit integers.
    ///
    /// This is a subtype of `any` and `eq`, and a supertype of `none`.
    I31,
    /// The abstract `array` heap type represents a reference to any kind of
    /// array.
    ///
    /// This is a subtype of `any` and `eq`, and a supertype of all concrete
    /// array types, as well as a supertype of the abstract `none` heap type.
    Array,
    /// A reference to an array of a specific, concrete type.
    ///
    /// These are subtypes of the `array` heap type (therefore also a subtype of
    /// `any` and `eq`) and supertypes of the `none` heap type.
    ConcreteArray(ArrayType),
    /// The abstract `struct` heap type represents a reference to any kind of
    /// struct.
    ///
    /// This is a subtype of `any` and `eq`, and a supertype of all concrete
    /// struct types, as well as a supertype of the abstract `none` heap type.
    Struct,
    /// A reference to an struct of a specific, concrete type.
    ///
    /// These are subtypes of the `struct` heap type (therefore also a subtype
    /// of `any` and `eq`) and supertypes of the `none` heap type.
    ConcreteStruct(StructType),
    /// The abstract `none` heap type represents the null internal reference.
    ///
    /// This is the bottom type for the internal type hierarchy, and therefore
    /// `none` is a subtype of internal types.
    None,

    /// The abstract `exn` heap type represents exception references.
    ///
    /// This is the top type for the exception type hierarchy, and therefore is
    /// the common supertype of all exception reference types.
    Exn,
    /// The abstract `noexn` heap type represents the null exception reference.
    ///
    /// This is the bottom type for the exception type hierarchy, and therefore
    /// is the common subtype of all exception reference types.
    NoExn,

    /// The abstract `cont` heap type represents continuation references.
    ///
    /// This is the top type for the continuation type hierarchy, and therefore is
    /// the common supertype of all continuation reference types.
    Cont,
    /// The abstract `nocont` heap type represents the null continuation reference.
    ///
    /// This is the bottom type for the continuation type hierarchy, and therefore
    /// is the common subtype of all continuation reference types.
    NoCont,
}

impl Display for HeapType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.shared {
            f.write_str("shared ")?;
        }
        match &self.inner {
            HeapTypeInner::Extern => write!(f, "extern"),
            HeapTypeInner::NoExtern => write!(f, "noextern"),
            HeapTypeInner::Func => write!(f, "func"),
            HeapTypeInner::NoFunc => write!(f, "nofunc"),
            HeapTypeInner::Any => write!(f, "any"),
            HeapTypeInner::Eq => write!(f, "eq"),
            HeapTypeInner::I31 => write!(f, "i31"),
            HeapTypeInner::Array => write!(f, "array"),
            HeapTypeInner::Struct => write!(f, "struct"),
            HeapTypeInner::None => write!(f, "none"),
            HeapTypeInner::ConcreteFunc(ty) => write!(f, "(concrete func {:?})", ty.type_index()),
            HeapTypeInner::ConcreteArray(ty) => write!(f, "(concrete array {:?})", ty.type_index()),
            HeapTypeInner::ConcreteStruct(ty) => {
                write!(f, "(concrete struct {:?})", ty.type_index())
            }
            HeapTypeInner::Exn => write!(f, "exn"),
            HeapTypeInner::NoExn => write!(f, "noexn"),
            HeapTypeInner::Cont => write!(f, "cont"),
            HeapTypeInner::NoCont => write!(f, "nocont"),
        }
    }
}

impl HeapType {
    pub const FUNC: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::Func,
    };
    pub const NOFUNC: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::NoFunc,
    };
    pub const EXTERN: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::Extern,
    };
    pub const NOEXTERN: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::NoExtern,
    };
    pub const ANY: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::Any,
    };
    pub const EQ: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::Eq,
    };
    pub const I31: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::I31,
    };
    pub const ARRAY: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::Array,
    };
    pub const STRUCT: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::Struct,
    };
    pub const NONE: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::None,
    };
    pub const EXN: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::Exn,
    };
    pub const NOEXN: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::NoExn,
    };
    pub const CONT: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::Cont,
    };
    pub const NOCONT: Self = HeapType {
        shared: false,
        inner: HeapTypeInner::NoCont,
    };
    pub const fn concrete_func(f: FuncType) -> HeapType {
        HeapType {
            shared: false,
            inner: HeapTypeInner::ConcreteFunc(f),
        }
    }

    /// Is this the abstract `extern` heap type?
    pub fn is_extern(&self) -> bool {
        matches!(self.inner, HeapTypeInner::Extern)
    }

    /// Is this the abstract `func` heap type?
    pub fn is_func(&self) -> bool {
        matches!(self.inner, HeapTypeInner::Func)
    }

    /// Is this the abstract `nofunc` heap type?
    pub fn is_no_func(&self) -> bool {
        matches!(self.inner, HeapTypeInner::NoFunc)
    }

    /// Is this the abstract `any` heap type?
    pub fn is_any(&self) -> bool {
        matches!(self.inner, HeapTypeInner::Any)
    }

    /// Is this the abstract `i31` heap type?
    pub fn is_i31(&self) -> bool {
        matches!(self.inner, HeapTypeInner::I31)
    }

    /// Is this the abstract `none` heap type?
    pub fn is_none(&self) -> bool {
        matches!(self.inner, HeapTypeInner::None)
    }

    /// Is this an abstract type?
    ///
    /// Types that are not abstract are concrete, user-defined types.
    pub fn is_abstract(&self) -> bool {
        !self.is_concrete()
    }

    /// Is this a concrete, user-defined heap type?
    ///
    /// Types that are not concrete, user-defined types are abstract types.
    #[inline]
    pub fn is_concrete(&self) -> bool {
        matches!(
            self.inner,
            HeapTypeInner::ConcreteFunc(_)
                | HeapTypeInner::ConcreteArray(_)
                | HeapTypeInner::ConcreteStruct(_)
        )
    }

    /// Is this a concrete, user-defined function type?
    pub fn is_concrete_func(&self) -> bool {
        matches!(self.inner, HeapTypeInner::ConcreteFunc(_))
    }

    /// Get the underlying concrete, user-defined function type, if any.
    ///
    /// Returns `None` if this is not a concrete function type.
    pub fn as_concrete_func(&self) -> Option<&FuncType> {
        match &self.inner {
            HeapTypeInner::ConcreteFunc(f) => Some(f),
            _ => None,
        }
    }

    /// Get the underlying concrete, user-defined type, panicking if this is not
    /// a concrete function type.
    pub fn unwrap_concrete_func(&self) -> &FuncType {
        self.as_concrete_func().unwrap()
    }

    /// Is this a concrete, user-defined array type?
    pub fn is_concrete_array(&self) -> bool {
        matches!(self.inner, HeapTypeInner::ConcreteArray(_))
    }

    /// Get the underlying concrete, user-defined array type, if any.
    ///
    /// Returns `None` for if this is not a concrete array type.
    pub fn as_concrete_array(&self) -> Option<&ArrayType> {
        match &self.inner {
            HeapTypeInner::ConcreteArray(f) => Some(f),
            _ => None,
        }
    }

    /// Get the underlying concrete, user-defined type, panicking if this is not
    /// a concrete array type.
    pub fn unwrap_concrete_array(&self) -> &ArrayType {
        self.as_concrete_array().unwrap()
    }

    /// Is this a concrete, user-defined struct type?
    pub fn is_concrete_struct(&self) -> bool {
        matches!(self.inner, HeapTypeInner::ConcreteStruct(_))
    }

    /// Get the underlying concrete, user-defined struct type, if any.
    ///
    /// Returns `None` for if this is not a concrete struct type.
    pub fn as_concrete_struct(&self) -> Option<&StructType> {
        match &self.inner {
            HeapTypeInner::ConcreteStruct(f) => Some(f),
            _ => None,
        }
    }

    /// Get the underlying concrete, user-defined type, panicking if this is not
    /// a concrete struct type.
    pub fn unwrap_concrete_struct(&self) -> &StructType {
        self.as_concrete_struct().unwrap()
    }

    pub fn is_shared(&self) -> bool {
        self.shared
    }

    /// Get the top type of this heap type's type hierarchy.
    ///
    /// The returned heap type is a supertype of all types in this heap type's
    /// type hierarchy.
    pub fn top(&self) -> HeapType {
        let inner = match self.inner {
            HeapTypeInner::Func | HeapTypeInner::ConcreteFunc(_) | HeapTypeInner::NoFunc => {
                HeapTypeInner::Func
            }

            HeapTypeInner::Extern | HeapTypeInner::NoExtern => HeapTypeInner::Extern,

            HeapTypeInner::Any
            | HeapTypeInner::Eq
            | HeapTypeInner::I31
            | HeapTypeInner::Array
            | HeapTypeInner::ConcreteArray(_)
            | HeapTypeInner::Struct
            | HeapTypeInner::ConcreteStruct(_)
            | HeapTypeInner::None => HeapTypeInner::Any,

            HeapTypeInner::Exn | HeapTypeInner::NoExn => HeapTypeInner::Exn,
            HeapTypeInner::Cont | HeapTypeInner::NoCont => HeapTypeInner::Cont,
        };

        HeapType {
            shared: self.shared,
            inner,
        }
    }

    /// Is this the top type within its type hierarchy?
    #[inline]
    pub fn is_top(&self) -> bool {
        matches!(
            self.inner,
            HeapTypeInner::Any
                | HeapTypeInner::Extern
                | HeapTypeInner::Func
                | HeapTypeInner::Exn
                | HeapTypeInner::Cont
        )
    }

    /// Get the bottom type of this heap type's type hierarchy.
    ///
    /// The returned heap type is a subtype of all types in this heap type's
    /// type hierarchy.
    pub fn bottom(&self) -> HeapType {
        let inner = match self.inner {
            HeapTypeInner::Extern | HeapTypeInner::NoExtern => HeapTypeInner::NoExtern,

            HeapTypeInner::Func | HeapTypeInner::ConcreteFunc(_) | HeapTypeInner::NoFunc => {
                HeapTypeInner::NoFunc
            }

            HeapTypeInner::Any
            | HeapTypeInner::Eq
            | HeapTypeInner::I31
            | HeapTypeInner::Array
            | HeapTypeInner::ConcreteArray(_)
            | HeapTypeInner::Struct
            | HeapTypeInner::ConcreteStruct(_)
            | HeapTypeInner::None => HeapTypeInner::None,

            HeapTypeInner::Exn | HeapTypeInner::NoExn => HeapTypeInner::NoExn,
            HeapTypeInner::Cont | HeapTypeInner::NoCont => HeapTypeInner::NoCont,
        };

        HeapType {
            shared: self.shared,
            inner,
        }
    }

    /// Is this the bottom type within its type hierarchy?
    #[inline]
    pub fn is_bottom(&self) -> bool {
        matches!(
            self.inner,
            HeapTypeInner::None
                | HeapTypeInner::NoExtern
                | HeapTypeInner::NoFunc
                | HeapTypeInner::NoExn
                | HeapTypeInner::NoCont
        )
    }

    /// Does this heap type match the other heap type?
    ///
    /// That is, is this heap type a subtype of the other?
    ///
    /// # Panics
    ///
    /// Panics if either type is associated with a different engine from the
    /// other.
    pub fn matches(&self, other: &HeapType) -> bool {
        match (&self.inner, &other.inner) {
            (HeapTypeInner::Extern, HeapTypeInner::Extern) => true,
            (HeapTypeInner::Extern, _) => false,

            (HeapTypeInner::NoExtern, HeapTypeInner::NoExtern | HeapTypeInner::Extern) => true,
            (HeapTypeInner::NoExtern, _) => false,

            (
                HeapTypeInner::NoFunc,
                HeapTypeInner::NoFunc | HeapTypeInner::ConcreteFunc(_) | HeapTypeInner::Func,
            ) => true,
            (HeapTypeInner::NoFunc, _) => false,

            (HeapTypeInner::ConcreteFunc(_), HeapTypeInner::Func) => true,
            (HeapTypeInner::ConcreteFunc(a), HeapTypeInner::ConcreteFunc(b)) => {
                assert!(a.comes_from_same_engine(b.engine()));
                a.engine()
                    .type_registry()
                    .is_subtype(a.type_index(), b.type_index())
            }
            (HeapTypeInner::ConcreteFunc(_), _) => false,

            (HeapTypeInner::Func, HeapTypeInner::Func) => true,
            (HeapTypeInner::Func, _) => false,

            (
                HeapTypeInner::None,
                HeapTypeInner::None
                | HeapTypeInner::ConcreteArray(_)
                | HeapTypeInner::Array
                | HeapTypeInner::ConcreteStruct(_)
                | HeapTypeInner::Struct
                | HeapTypeInner::I31
                | HeapTypeInner::Eq
                | HeapTypeInner::Any,
            ) => true,
            (HeapTypeInner::None, _) => false,

            (
                HeapTypeInner::ConcreteArray(_),
                HeapTypeInner::Array | HeapTypeInner::Eq | HeapTypeInner::Any,
            ) => true,
            (HeapTypeInner::ConcreteArray(a), HeapTypeInner::ConcreteArray(b)) => {
                assert!(a.comes_from_same_engine(b.engine()));
                a.engine()
                    .type_registry()
                    .is_subtype(a.type_index(), b.type_index())
            }
            (HeapTypeInner::ConcreteArray(_), _) => false,

            (
                HeapTypeInner::Array,
                HeapTypeInner::Array | HeapTypeInner::Eq | HeapTypeInner::Any,
            ) => true,
            (HeapTypeInner::Array, _) => false,

            (
                HeapTypeInner::ConcreteStruct(_),
                HeapTypeInner::Struct | HeapTypeInner::Eq | HeapTypeInner::Any,
            ) => true,
            (HeapTypeInner::ConcreteStruct(a), HeapTypeInner::ConcreteStruct(b)) => {
                assert!(a.comes_from_same_engine(b.engine()));
                a.engine()
                    .type_registry()
                    .is_subtype(a.type_index(), b.type_index())
            }
            (HeapTypeInner::ConcreteStruct(_), _) => false,

            (
                HeapTypeInner::Struct,
                HeapTypeInner::Struct | HeapTypeInner::Eq | HeapTypeInner::Any,
            ) => true,
            (HeapTypeInner::Struct, _) => false,

            (HeapTypeInner::I31, HeapTypeInner::I31 | HeapTypeInner::Eq | HeapTypeInner::Any) => {
                true
            }
            (HeapTypeInner::I31, _) => false,

            (HeapTypeInner::Eq, HeapTypeInner::Eq | HeapTypeInner::Any) => true,
            (HeapTypeInner::Eq, _) => false,

            (HeapTypeInner::Any, HeapTypeInner::Any) => true,
            (HeapTypeInner::Any, _) => false,

            (HeapTypeInner::Exn, HeapTypeInner::Exn) => true,
            (HeapTypeInner::Exn, _) => false,
            (HeapTypeInner::NoExn, HeapTypeInner::NoExn | HeapTypeInner::Exn) => true,
            (HeapTypeInner::NoExn, _) => false,

            (HeapTypeInner::Cont, HeapTypeInner::Cont) => true,
            (HeapTypeInner::Cont, _) => false,
            // (
            //     HeapTypeInner::ConcreteCont(_),
            //     HeapTypeInner::Cont,
            // ) => true,
            // (HeapTypeInner::ConcreteCont(_a), HeapTypeInner::ConcreteCont(_b)) => {
            //     // assert!(a.comes_from_same_engine(b.engine()));
            //     // a.engine()
            //     //     .type_registry()
            //     //     .is_subtype(a.type_index(), b.type_index())
            //     todo!()
            // }
            // (HeapTypeInner::ConcreteCont(_), _) => false,

            (HeapTypeInner::NoCont, HeapTypeInner::NoCont | HeapTypeInner::Cont) => {
                true
            }
            (HeapTypeInner::NoCont, _) => false,
        }
    }

    /// Is heap type `a` precisely equal to heap type `b`?
    ///
    /// Returns `false` even if `a` is a subtype of `b` or vice versa, if they
    /// are not exactly the same heap type.
    ///
    /// # Panics
    ///
    /// Panics if either type is associated with a different engine from the
    /// other.
    pub fn eq(a: &HeapType, b: &HeapType) -> bool {
        a.matches(b) && b.matches(a)
    }

    pub(crate) fn ensure_matches(&self, engine: &Engine, other: &HeapType) -> crate::Result<()> {
        if !self.comes_from_same_engine(engine) || !other.comes_from_same_engine(engine) {
            bail!("type used with wrong engine");
        }
        if self.matches(other) {
            Ok(())
        } else {
            bail!("type mismatch: expected {other}, found {self}");
        }
    }

    pub(crate) fn comes_from_same_engine(&self, engine: &Engine) -> bool {
        match &self.inner {
            HeapTypeInner::Extern
            | HeapTypeInner::NoExtern
            | HeapTypeInner::Func
            | HeapTypeInner::NoFunc
            | HeapTypeInner::Any
            | HeapTypeInner::Eq
            | HeapTypeInner::I31
            | HeapTypeInner::Array
            | HeapTypeInner::Struct
            | HeapTypeInner::None
            | HeapTypeInner::Exn
            | HeapTypeInner::NoExn
            | HeapTypeInner::Cont
            | HeapTypeInner::NoCont => true,
            HeapTypeInner::ConcreteFunc(ty) => ty.comes_from_same_engine(engine),
            HeapTypeInner::ConcreteArray(ty) => ty.comes_from_same_engine(engine),
            HeapTypeInner::ConcreteStruct(ty) => ty.comes_from_same_engine(engine),
        }
    }

    pub(crate) fn to_wasm_type(&self) -> WasmHeapType {
        let inner = match &self.inner {
            HeapTypeInner::Extern => WasmHeapTypeInner::Extern,
            HeapTypeInner::NoExtern => WasmHeapTypeInner::NoExtern,
            HeapTypeInner::Func => WasmHeapTypeInner::Func,
            HeapTypeInner::NoFunc => WasmHeapTypeInner::NoFunc,
            HeapTypeInner::Any => WasmHeapTypeInner::Any,
            HeapTypeInner::Eq => WasmHeapTypeInner::Eq,
            HeapTypeInner::I31 => WasmHeapTypeInner::I31,
            HeapTypeInner::Array => WasmHeapTypeInner::Array,
            HeapTypeInner::Struct => WasmHeapTypeInner::Struct,
            HeapTypeInner::None => WasmHeapTypeInner::None,
            HeapTypeInner::ConcreteFunc(f) => {
                WasmHeapTypeInner::ConcreteFunc(CanonicalizedTypeIndex::Engine(f.type_index()))
            }
            HeapTypeInner::ConcreteArray(a) => {
                WasmHeapTypeInner::ConcreteArray(CanonicalizedTypeIndex::Engine(a.type_index()))
            }
            HeapTypeInner::ConcreteStruct(a) => {
                WasmHeapTypeInner::ConcreteStruct(CanonicalizedTypeIndex::Engine(a.type_index()))
            }
            HeapTypeInner::Exn => WasmHeapTypeInner::Exn,
            HeapTypeInner::NoExn => WasmHeapTypeInner::NoExn,
            HeapTypeInner::Cont => WasmHeapTypeInner::Cont,
            HeapTypeInner::NoCont => WasmHeapTypeInner::NoCont,
        };
        WasmHeapType {
            shared: self.shared,
            inner,
        }
    }

    pub(crate) fn from_wasm_type(engine: &Engine, ty: &WasmHeapType) -> HeapType {
        let inner = match ty.inner {
            WasmHeapTypeInner::Extern => HeapTypeInner::Extern,
            WasmHeapTypeInner::NoExtern => HeapTypeInner::NoExtern,
            WasmHeapTypeInner::Func => HeapTypeInner::Func,
            WasmHeapTypeInner::NoFunc => HeapTypeInner::NoFunc,
            WasmHeapTypeInner::Any => HeapTypeInner::Any,
            WasmHeapTypeInner::Eq => HeapTypeInner::Eq,
            WasmHeapTypeInner::I31 => HeapTypeInner::I31,
            WasmHeapTypeInner::Array => HeapTypeInner::Array,
            WasmHeapTypeInner::Struct => HeapTypeInner::Struct,
            WasmHeapTypeInner::None => HeapTypeInner::None,
            WasmHeapTypeInner::ConcreteFunc(CanonicalizedTypeIndex::Engine(idx)) => {
                HeapTypeInner::ConcreteFunc(FuncType::from_shared_type_index(engine, idx))
            }
            WasmHeapTypeInner::ConcreteArray(CanonicalizedTypeIndex::Engine(idx)) => {
                HeapTypeInner::ConcreteArray(ArrayType::from_shared_type_index(engine, idx))
            }
            WasmHeapTypeInner::ConcreteStruct(CanonicalizedTypeIndex::Engine(idx)) => {
                HeapTypeInner::ConcreteStruct(StructType::from_shared_type_index(engine, idx))
            }

            WasmHeapTypeInner::ConcreteFunc(CanonicalizedTypeIndex::Module(_))
            | WasmHeapTypeInner::ConcreteFunc(CanonicalizedTypeIndex::RecGroup(_))
            | WasmHeapTypeInner::ConcreteArray(CanonicalizedTypeIndex::Module(_))
            | WasmHeapTypeInner::ConcreteArray(CanonicalizedTypeIndex::RecGroup(_))
            | WasmHeapTypeInner::ConcreteStruct(CanonicalizedTypeIndex::Module(_))
            | WasmHeapTypeInner::ConcreteStruct(CanonicalizedTypeIndex::RecGroup(_)) => {
                panic!(
                    "HeapTypeInner::from_wasm_type on non-canonicalized-for-runtime-usage heap type"
                )
            }

            WasmHeapTypeInner::Exn => HeapTypeInner::Exn,
            WasmHeapTypeInner::NoExn => HeapTypeInner::NoExn,

            WasmHeapTypeInner::Cont => HeapTypeInner::Cont,
            WasmHeapTypeInner::ConcreteCont(_) => todo!(),
            WasmHeapTypeInner::NoCont => HeapTypeInner::NoCont,
        };
        HeapType {
            inner,
            shared: ty.shared,
        }
    }
}

/// A list of all possible types which can be externally referenced from a
/// WebAssembly module.
///
/// This list can be found in [`ImportType`] or [`ExportType`], so these types
/// can either be imported or exported.
#[derive(Debug, Clone)]
pub enum ExternType {
    /// This external type is the type of a WebAssembly function.
    Func(FuncType),
    /// This external type is the type of a WebAssembly global.
    Global(GlobalType),
    /// This external type is the type of a WebAssembly table.
    Table(TableType),
    /// This external type is the type of a WebAssembly memory.
    Memory(MemoryType),
    /// This external type is the type of a WebAssembly tag.
    Tag(TagType),
}

/// The storage type of a `struct` field or `array` element.
///
/// This is either a packed 8- or -16 bit integer, or else it is some unpacked
/// Wasm value type.
#[derive(Clone, Hash)]
pub enum StorageType {
    /// `i8`, an 8-bit integer.
    I8,
    /// `i16`, a 16-bit integer.
    I16,
    /// A value type.
    ValType(ValType),
}
impl fmt::Display for StorageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageType::I8 => write!(f, "i8"),
            StorageType::I16 => write!(f, "i16"),
            StorageType::ValType(ty) => fmt::Display::fmt(ty, f),
        }
    }
}

/// The type of a `struct` field or an `array`'s elements.
///
/// This is a pair of both the field's storage type and its mutability
/// (i.e. whether the field can be updated or not).
#[derive(Clone, Hash)]
pub struct FieldType {
    mutability: Mutability,
    element_type: StorageType,
}
impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mutability.is_var() {
            write!(f, "(mut {})", self.element_type)
        } else {
            fmt::Display::fmt(&self.element_type, f)
        }
    }
}

impl FieldType {
    /// Construct a new field type from the given parts.
    #[inline]
    pub fn new(mutability: Mutability, element_type: StorageType) -> Self {
        Self {
            mutability,
            element_type,
        }
    }

    /// Get whether or not this field type is mutable.
    #[inline]
    pub fn mutability(&self) -> Mutability {
        self.mutability
    }

    /// Get this field type's storage type.
    #[inline]
    pub fn element_type(&self) -> &StorageType {
        &self.element_type
    }
}

/// The type of a WebAssembly struct.
///
/// WebAssembly structs are a static, fixed-length, ordered sequence of
/// fields. Fields are named by index, not an identifier. Each field is mutable
/// or constant and stores unpacked [`Val`][crate::Val]s or packed 8-/16-bit
/// integers.
///
/// # Subtyping and Equality
///
/// `StructType` does not implement `Eq`, because reference types have a
/// subtyping relationship, and so 99.99% of the time you actually want to check
/// whether one type matches (i.e. is a subtype of) another type. You can use
/// the [`StructType::matches`] method to perform these types of checks. If,
/// however, you are in that 0.01% scenario where you need to check precise
/// equality between types, you can use the [`StructType::eq`] method.
//
// TODO: Once we have struct values, update above docs with a reference to the
// future `Struct::matches_ty` method
#[derive(Debug, Clone, Hash)]
pub struct StructType {
    registered_type: RegisteredType,
}
impl fmt::Display for StructType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(struct")?;
        for field in self.fields() {
            write!(f, " (field {field})")?;
        }
        write!(f, ")")?;
        Ok(())
    }
}

impl StructType {
    pub fn fields(&self) -> impl ExactSizeIterator<Item = FieldType> {
        core::iter::empty()
    }

    pub(super) fn engine(&self) -> &Engine {
        self.registered_type.engine()
    }
    pub(super) fn comes_from_same_engine(&self, other: &Engine) -> bool {
        Engine::same(self.engine(), other)
    }
    pub(crate) fn from_shared_type_index(engine: &Engine, index: VMSharedTypeIndex) -> StructType {
        todo!()
        // let ty = RegisteredType::root(engine, index);
        // Self::from_registered_type(ty)
    }
    pub(crate) fn from_registered_type(registered_type: RegisteredType) -> Self {
        debug_assert!(registered_type.is_struct());
        Self { registered_type }
    }
    pub(crate) fn type_index(&self) -> VMSharedTypeIndex {
        self.registered_type.index()
    }
}

/// The type of a WebAssembly array.
///
/// WebAssembly arrays are dynamically-sized, but not resizable. They contain
/// either unpacked [`Val`][crate::Val]s or packed 8-/16-bit integers.
///
/// # Subtyping and Equality
///
/// `ArrayType` does not implement `Eq`, because reference types have a
/// subtyping relationship, and so 99.99% of the time you actually want to check
/// whether one type matches (i.e. is a subtype of) another type. You can use
/// the [`ArrayType::matches`] method to perform these types of checks. If,
/// however, you are in that 0.01% scenario where you need to check precise
/// equality between types, you can use the [`ArrayType::eq`] method.
//
// TODO: Once we have array values, update above docs with a reference to the
// future `Array::matches_ty` method
#[derive(Debug, Clone, Hash)]
pub struct ArrayType {
    registered_type: RegisteredType,
}
impl fmt::Display for ArrayType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let field_ty = self.field_type();
        write!(f, "(array (field {field_ty}))")?;
        Ok(())
    }
}

impl ArrayType {
    pub fn field_type(&self) -> FieldType {
        todo!()
    }
    pub(crate) fn type_index(&self) -> VMSharedTypeIndex {
        self.registered_type.index()
    }
    pub(super) fn engine(&self) -> &Engine {
        self.registered_type.engine()
    }
    pub(super) fn comes_from_same_engine(&self, other: &Engine) -> bool {
        Engine::same(self.engine(), other)
    }
    pub(crate) fn from_shared_type_index(engine: &Engine, index: VMSharedTypeIndex) -> ArrayType {
        // let ty = RegisteredType::root(engine, index);
        // Self::from_registered_type(ty)

        todo!()
    }
    pub(crate) fn from_registered_type(registered_type: RegisteredType) -> Self {
        debug_assert!(registered_type.is_array());
        Self { registered_type }
    }
}

/// The type of a WebAssembly function.
///
/// WebAssembly functions can have 0 or more parameters and results.
///
/// # Subtyping and Equality
///
/// `FuncType` does not implement `Eq`, because reference types have a subtyping
/// relationship, and so 99.99% of the time you actually want to check whether
/// one type matches (i.e. is a subtype of) another type. You can use the
/// [`FuncType::matches`] and [`Func::matches_ty`][crate::Func::matches_ty]
/// methods to perform these types of checks. If, however, you are in that 0.01%
/// scenario where you need to check precise equality between types, you can use
/// the [`FuncType::eq`] method.
#[derive(Debug, Clone, Hash)]
pub struct FuncType {
    registered_type: RegisteredType,
}
impl Display for FuncType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(type (func")?;
        if self.params().len() > 0 {
            write!(f, " (param")?;
            for p in self.params() {
                write!(f, " {p}")?;
            }
            write!(f, ")")?;
        }
        if self.results().len() > 0 {
            write!(f, " (result")?;
            for r in self.results() {
                write!(f, " {r}")?;
            }
            write!(f, ")")?;
        }
        write!(f, "))")
    }
}

impl FuncType {
    pub fn params(&self) -> impl ExactSizeIterator<Item = ValType> + '_ {
        core::iter::empty()
    }
    pub fn results(&self) -> impl ExactSizeIterator<Item = ValType> + '_ {
        core::iter::empty()
    }
    pub(super) fn type_index(&self) -> VMSharedTypeIndex {
        self.registered_type.index()
    }
    pub(super) fn engine(&self) -> &Engine {
        self.registered_type.engine()
    }
    pub(super) fn comes_from_same_engine(&self, other: &Engine) -> bool {
        Engine::same(self.engine(), other)
    }
    pub(super) fn from_shared_type_index(engine: &Engine, index: VMSharedTypeIndex) -> FuncType {
        // let ty = RegisteredType::root(engine, index);
        // Self::from_registered_type(ty)
        todo!()
    }
    pub(super) fn from_registered_type(registered_type: RegisteredType) -> Self {
        debug_assert!(registered_type.is_func());
        Self { registered_type }
    }
}

/// A descriptor for a table in a WebAssembly module.
///
/// Tables are contiguous chunks of a specific element, typically a `funcref` or
/// an `externref`. The most common use for tables is a function table through
/// which `call_indirect` can invoke other functions.
#[derive(Debug, Clone, Hash)]
pub struct TableType {
    // Keep a `wasmtime::RefType` so that `TableType::element` doesn't need to
    // take an `&Engine`.
    element: RefType,
    ty: Table,
}

/// A descriptor for a WebAssembly memory type.
///
/// Memories are described in units of pages (64KB) and represent contiguous
/// chunks of addressable memory.
#[derive(Debug, Clone, Hash)]
pub struct MemoryType {
    ty: Memory,
}

/// A WebAssembly global descriptor.
///
/// This type describes an instance of a global in a WebAssembly module. Globals
/// are local to an [`Instance`](crate::Instance) and are either immutable or
/// mutable.
#[derive(Debug, Clone, Hash)]
pub struct GlobalType {
    content: ValType,
    mutability: Mutability,
}

/// A descriptor for a tag in a WebAssembly module.
///
/// This type describes an instance of a tag in a WebAssembly
/// module. Tags are local to an [`Instance`](crate::Instance).
#[derive(Debug, Clone, Hash)]
pub struct TagType {
    ty: FuncType,
}

/// A descriptor for an imported value into a wasm module.
///
/// This type is primarily accessed from the
/// [`Module::imports`](crate::Module::imports) API. Each [`ImportType`]
/// describes an import into the wasm module with the module/name that it's
/// imported from as well as the type of item that's being imported.
#[derive(Clone)]
pub struct ImportType<'module> {
    /// The module of the import.
    module: &'module str,

    /// The field of the import.
    name: &'module str,

    /// The type of the import.
    ty: EntityType,
    types: &'module ModuleTypes,
    engine: &'module Engine,
}

impl<'module> fmt::Debug for ImportType<'module> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportType")
            .field("module", &self.module)
            .field("name", &self.name)
            .field("ty", &self.ty)
            .finish()
    }
}

/// A descriptor for an exported WebAssembly value.
///
/// This type is primarily accessed from the
/// [`Module::exports`](crate::Module::exports) accessor and describes what
/// names are exported from a wasm module and the type of the item that is
/// exported.
#[derive(Clone)]
pub struct ExportType<'module> {
    /// The name of the export.
    name: &'module str,

    /// The type of the export.
    ty: EntityType,
    types: &'module ModuleTypes,
    engine: &'module Engine,
}

impl<'module> fmt::Debug for ExportType<'module> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExportType")
            .field("name", &self.name)
            .field("ty", &self.ty)
            .finish()
    }
}
