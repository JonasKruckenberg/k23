use crate::wasm::enum_accessors;
use crate::wasm::indices::CanonicalizedTypeIndex;
use crate::wasm::translate::{GlobalDesc, MemoryDesc, TableDesc};
use crate::wasm::type_registry::TypeTrace;
use alloc::boxed::Box;
use anyhow::bail;
use core::fmt;

/// Represents the types of values in a WebAssembly module.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WasmValType {
    /// The value type is i32.
    I32,
    /// The value type is i64.
    I64,
    /// The value type is f32.
    F32,
    /// The value type is f64.
    F64,
    /// The value type is v128.
    V128,
    /// The value type is a reference.
    Ref(WasmRefType),
}

impl WasmValType {
    pub fn is_i32(&self) -> bool {
        matches!(self, Self::I32)
    }
    pub fn is_i64(&self) -> bool {
        matches!(self, Self::I64)
    }
    pub fn is_f32(&self) -> bool {
        matches!(self, Self::F32)
    }
    pub fn is_f64(&self) -> bool {
        matches!(self, Self::F64)
    }
    pub fn is_v128(&self) -> bool {
        matches!(self, Self::V128)
    }
    enum_accessors!(
        e
        (Ref(&WasmRefType) is_ref get_ref unwrap_ref e)
    );

    pub fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::I32, Self::I32) => true,
            (Self::I64, Self::I64) => true,
            (Self::F32, Self::F32) => true,
            (Self::F64, Self::F64) => true,
            (Self::V128, Self::V128) => true,
            (Self::Ref(a), Self::Ref(b)) => a.matches(b),
            (Self::I32 | Self::I64 | Self::F32 | Self::F64 | Self::V128 | Self::Ref(_), _) => false,
        }
    }

    pub fn ensure_matches(&self, other: &Self) -> crate::Result<()> {
        if self.matches(other) {
            Ok(())
        } else {
            bail!("type mismatch: expected {other}, found {self}");
        }
    }
}

impl fmt::Display for WasmValType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WasmValType::I32 => write!(f, "i32"),
            WasmValType::I64 => write!(f, "i64"),
            WasmValType::F32 => write!(f, "f32"),
            WasmValType::F64 => write!(f, "f64"),
            WasmValType::V128 => write!(f, "v128"),
            WasmValType::Ref(rt) => write!(f, "{rt}"),
        }
    }
}

impl TypeTrace for WasmValType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        if let Self::Ref(ref_type) = self {
            ref_type.trace(func)
        } else {
            Ok(())
        }
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        if let Self::Ref(ref_type) = self {
            ref_type.trace_mut(func)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmRefType {
    pub nullable: bool,
    pub heap_type: WasmHeapType,
}

impl WasmRefType {
    pub const EXTERNREF: WasmRefType = WasmRefType {
        nullable: true,
        heap_type: WasmHeapType::new(false, WasmHeapTypeInner::Extern),
    };
    pub const FUNCREF: WasmRefType = WasmRefType {
        nullable: true,
        heap_type: WasmHeapType::new(false, WasmHeapTypeInner::Func),
    };

    /// Is this a type that is represented as a `VMGcRef`?
    #[inline]
    pub fn is_vmgcref_type(&self) -> bool {
        self.heap_type.is_vmgcref_type()
    }

    /// Is this a type that is represented as a `VMGcRef` and is additionally
    /// not an `i31`?
    ///
    /// That is, is this a a type that actually refers to an object allocated in
    /// a GC heap?
    #[inline]
    pub fn is_vmgcref_type_and_not_i31(&self) -> bool {
        self.heap_type.is_vmgcref_type_and_not_i31()
    }

    pub fn matches(&self, other: &Self) -> bool {
        if self.nullable && !other.nullable {
            return false;
        }
        self.heap_type.matches(&other.heap_type)
    }

    pub(crate) fn ensure_matches(&self, other: &Self) -> crate::Result<()> {
        if self.matches(other) {
            Ok(())
        } else {
            bail!("type mismatch: expected {other}, found {self}");
        }
    }
}

impl fmt::Display for WasmRefType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Self::FUNCREF => write!(f, "funcref"),
            Self::EXTERNREF => write!(f, "externref"),
            _ => {
                if self.nullable {
                    write!(f, "(ref null {})", self.heap_type)
                } else {
                    write!(f, "(ref {})", self.heap_type)
                }
            }
        }
    }
}

impl TypeTrace for WasmRefType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        self.heap_type.trace(func)
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        self.heap_type.trace_mut(func)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmHeapType {
    pub shared: bool,
    pub inner: WasmHeapTypeInner,
}

impl WasmHeapType {
    pub const fn new(shared: bool, ty: WasmHeapTypeInner) -> Self {
        Self { shared, inner: ty }
    }

    /// Is this a type that is represented as a `VMGcRef`?
    #[inline]
    pub fn is_vmgcref_type(&self) -> bool {
        match self.top().inner {
            // All `t <: (ref null any)` and `t <: (ref null extern)` are
            // represented as `VMGcRef`s.
            WasmHeapTypeInner::Any | WasmHeapTypeInner::Extern => true,
            // All others are not.
            _ => false,
        }
    }

    /// Is this a type that is represented as a `VMGcRef` and is additionally
    /// not an `i31`?
    ///
    /// That is, is this a a type that actually refers to an object allocated in
    /// a GC heap?
    #[inline]
    pub fn is_vmgcref_type_and_not_i31(&self) -> bool {
        self.is_vmgcref_type() && self.inner != WasmHeapTypeInner::I31
    }

    /// Get the top type of this heap type's type hierarchy.
    ///
    /// The returned heap type is a supertype of all types in this heap type's
    /// type hierarchy.
    pub fn top(&self) -> Self {
        let ty = match self.inner {
            WasmHeapTypeInner::Func
            | WasmHeapTypeInner::ConcreteFunc(_)
            | WasmHeapTypeInner::NoFunc => WasmHeapTypeInner::Func,

            WasmHeapTypeInner::Extern | WasmHeapTypeInner::NoExtern => WasmHeapTypeInner::Extern,

            WasmHeapTypeInner::Any
            | WasmHeapTypeInner::Eq
            | WasmHeapTypeInner::I31
            | WasmHeapTypeInner::Array
            | WasmHeapTypeInner::ConcreteArray(_)
            | WasmHeapTypeInner::Struct
            | WasmHeapTypeInner::ConcreteStruct(_)
            | WasmHeapTypeInner::None => WasmHeapTypeInner::Any,

            WasmHeapTypeInner::Exn | WasmHeapTypeInner::NoExn => WasmHeapTypeInner::Exn,
            WasmHeapTypeInner::Cont | WasmHeapTypeInner::NoCont => WasmHeapTypeInner::Cont,
        };

        Self {
            shared: self.shared,
            inner: ty,
        }
    }

    /// Is this the top type within its type hierarchy?
    #[inline]
    pub fn is_top(&self) -> bool {
        matches!(
            self.inner,
            WasmHeapTypeInner::Any
                | WasmHeapTypeInner::Extern
                | WasmHeapTypeInner::Func
                | WasmHeapTypeInner::Exn
                | WasmHeapTypeInner::Cont
        )
    }

    /// Get the bottom type of this heap type's type hierarchy.
    ///
    /// The returned heap type is a subtype of all types in this heap type's
    /// type hierarchy.
    #[inline]
    pub fn bottom(&self) -> Self {
        let ty = match self.inner {
            WasmHeapTypeInner::Extern | WasmHeapTypeInner::NoExtern => WasmHeapTypeInner::NoExtern,

            WasmHeapTypeInner::Func
            | WasmHeapTypeInner::ConcreteFunc(_)
            | WasmHeapTypeInner::NoFunc => WasmHeapTypeInner::NoFunc,

            WasmHeapTypeInner::Any
            | WasmHeapTypeInner::Eq
            | WasmHeapTypeInner::I31
            | WasmHeapTypeInner::Array
            | WasmHeapTypeInner::ConcreteArray(_)
            | WasmHeapTypeInner::Struct
            | WasmHeapTypeInner::ConcreteStruct(_)
            | WasmHeapTypeInner::None => WasmHeapTypeInner::None,

            WasmHeapTypeInner::Exn | WasmHeapTypeInner::NoExn => WasmHeapTypeInner::NoExn,
            WasmHeapTypeInner::Cont | WasmHeapTypeInner::NoCont => WasmHeapTypeInner::NoCont,
        };

        Self {
            shared: self.shared,
            inner: ty,
        }
    }

    /// Is this the bottom type within its type hierarchy?
    #[inline]
    pub fn is_bottom(&self) -> bool {
        matches!(
            self.inner,
            WasmHeapTypeInner::None
                | WasmHeapTypeInner::NoExtern
                | WasmHeapTypeInner::NoFunc
                | WasmHeapTypeInner::NoExn
                | WasmHeapTypeInner::NoCont
        )
    }

    /// Is this a concrete, user-defined heap type?
    ///
    /// Types that are not concrete, user-defined types are abstract types.
    #[inline]
    pub fn is_concrete(&self) -> bool {
        matches!(
            self.inner,
            WasmHeapTypeInner::ConcreteFunc(_)
                | WasmHeapTypeInner::ConcreteArray(_)
                | WasmHeapTypeInner::ConcreteStruct(_)
        )
    }

    pub fn matches(&self, other: &WasmHeapType) -> bool {
        match (&self.inner, &other.inner) {
            (WasmHeapTypeInner::Extern, WasmHeapTypeInner::Extern) => true,
            (WasmHeapTypeInner::Extern, _) => false,

            (
                WasmHeapTypeInner::NoExtern,
                WasmHeapTypeInner::NoExtern | WasmHeapTypeInner::Extern,
            ) => true,
            (WasmHeapTypeInner::NoExtern, _) => false,

            (
                WasmHeapTypeInner::NoFunc,
                WasmHeapTypeInner::NoFunc
                | WasmHeapTypeInner::ConcreteFunc(_)
                | WasmHeapTypeInner::Func,
            ) => true,
            (WasmHeapTypeInner::NoFunc, _) => false,

            (WasmHeapTypeInner::ConcreteFunc(_), WasmHeapTypeInner::Func) => true,
            (WasmHeapTypeInner::ConcreteFunc(_a), WasmHeapTypeInner::ConcreteFunc(_b)) => {
                // assert!(a.comes_from_same_engine(b.engine()));
                // a.engine()
                //     .signatures()
                //     .is_subtype(a.type_index(), b.type_index())

                todo!()
            }
            (WasmHeapTypeInner::ConcreteFunc(_), _) => false,

            (WasmHeapTypeInner::Func, WasmHeapTypeInner::Func) => true,
            (WasmHeapTypeInner::Func, _) => false,

            (
                WasmHeapTypeInner::None,
                WasmHeapTypeInner::None
                | WasmHeapTypeInner::ConcreteArray(_)
                | WasmHeapTypeInner::Array
                | WasmHeapTypeInner::ConcreteStruct(_)
                | WasmHeapTypeInner::Struct
                | WasmHeapTypeInner::I31
                | WasmHeapTypeInner::Eq
                | WasmHeapTypeInner::Any,
            ) => true,
            (WasmHeapTypeInner::None, _) => false,

            (
                WasmHeapTypeInner::ConcreteArray(_),
                WasmHeapTypeInner::Array | WasmHeapTypeInner::Eq | WasmHeapTypeInner::Any,
            ) => true,
            (WasmHeapTypeInner::ConcreteArray(_a), WasmHeapTypeInner::ConcreteArray(_b)) => {
                // assert!(a.comes_from_same_engine(b.engine()));
                // a.engine()
                //     .signatures()
                //     .is_subtype(a.type_index(), b.type_index())

                todo!()
            }
            (WasmHeapTypeInner::ConcreteArray(_), _) => false,

            (
                WasmHeapTypeInner::Array,
                WasmHeapTypeInner::Array | WasmHeapTypeInner::Eq | WasmHeapTypeInner::Any,
            ) => true,
            (WasmHeapTypeInner::Array, _) => false,

            (
                WasmHeapTypeInner::ConcreteStruct(_),
                WasmHeapTypeInner::Struct | WasmHeapTypeInner::Eq | WasmHeapTypeInner::Any,
            ) => true,
            (WasmHeapTypeInner::ConcreteStruct(_a), WasmHeapTypeInner::ConcreteStruct(_b)) => {
                // assert!(a.comes_from_same_engine(b.engine()));
                // a.engine()
                //     .signatures()
                //     .is_subtype(a.type_index(), b.type_index())
                todo!()
            }
            (WasmHeapTypeInner::ConcreteStruct(_), _) => false,

            (
                WasmHeapTypeInner::Struct,
                WasmHeapTypeInner::Struct | WasmHeapTypeInner::Eq | WasmHeapTypeInner::Any,
            ) => true,
            (WasmHeapTypeInner::Struct, _) => false,

            (
                WasmHeapTypeInner::I31,
                WasmHeapTypeInner::I31 | WasmHeapTypeInner::Eq | WasmHeapTypeInner::Any,
            ) => true,
            (WasmHeapTypeInner::I31, _) => false,

            (WasmHeapTypeInner::Eq, WasmHeapTypeInner::Eq | WasmHeapTypeInner::Any) => true,
            (WasmHeapTypeInner::Eq, _) => false,

            (WasmHeapTypeInner::Any, WasmHeapTypeInner::Any) => true,
            (WasmHeapTypeInner::Any, _) => false,

            (WasmHeapTypeInner::Exn, WasmHeapTypeInner::Exn) => true,
            (WasmHeapTypeInner::Exn, _) => false,
            (WasmHeapTypeInner::NoExn, WasmHeapTypeInner::NoExn | WasmHeapTypeInner::Exn) => true,
            (WasmHeapTypeInner::NoExn, _) => false,

            (WasmHeapTypeInner::Cont, WasmHeapTypeInner::Cont) => true,
            (WasmHeapTypeInner::Cont, _) => false,
            (WasmHeapTypeInner::NoCont, WasmHeapTypeInner::NoCont | WasmHeapTypeInner::Cont) => {
                true
            }
            (WasmHeapTypeInner::NoCont, _) => false,
        }
    }

    pub fn ensure_matches(&self, other: &WasmHeapType) -> crate::Result<()> {
        if self.matches(other) {
            Ok(())
        } else {
            bail!("type mismatch: expected {other}, found {self}");
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WasmHeapTypeInner {
    // External types.
    Extern,
    NoExtern,

    // Function types.
    Func,
    ConcreteFunc(CanonicalizedTypeIndex),
    NoFunc,

    // Internal types.
    Any,
    Eq,
    I31,
    Array,
    ConcreteArray(CanonicalizedTypeIndex),
    Struct,
    ConcreteStruct(CanonicalizedTypeIndex),
    None,

    Exn,
    NoExn,

    Cont,
    NoCont,
}

impl fmt::Display for WasmHeapType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.shared {
            write!(f, "shared ")?;
        }
        match &self.inner {
            WasmHeapTypeInner::Extern => write!(f, "extern"),
            WasmHeapTypeInner::NoExtern => write!(f, "noextern"),
            WasmHeapTypeInner::Func => write!(f, "func"),
            WasmHeapTypeInner::ConcreteFunc(i) => write!(f, "func {i:?}"),
            WasmHeapTypeInner::NoFunc => write!(f, "nofunc"),
            WasmHeapTypeInner::Any => write!(f, "any"),
            WasmHeapTypeInner::Eq => write!(f, "eq"),
            WasmHeapTypeInner::I31 => write!(f, "i31"),
            WasmHeapTypeInner::Array => write!(f, "array"),
            WasmHeapTypeInner::ConcreteArray(i) => write!(f, "array {i:?}"),
            WasmHeapTypeInner::Struct => write!(f, "struct"),
            WasmHeapTypeInner::ConcreteStruct(i) => write!(f, "struct {i:?}"),
            WasmHeapTypeInner::None => write!(f, "none"),
            WasmHeapTypeInner::Exn => write!(f, "exn"),
            WasmHeapTypeInner::NoExn => write!(f, "noexn"),
            WasmHeapTypeInner::Cont => write!(f, "cont"),
            WasmHeapTypeInner::NoCont => write!(f, "nocont"),
        }
    }
}

impl TypeTrace for WasmHeapType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        match self.inner {
            WasmHeapTypeInner::ConcreteFunc(index)
            | WasmHeapTypeInner::ConcreteArray(index)
            | WasmHeapTypeInner::ConcreteStruct(index) => func(index),
            _ => Ok(()),
        }
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        match self.inner {
            WasmHeapTypeInner::ConcreteFunc(ref mut index)
            | WasmHeapTypeInner::ConcreteArray(ref mut index)
            | WasmHeapTypeInner::ConcreteStruct(ref mut index) => func(index),
            _ => Ok(()),
        }
    }
}

impl WasmHeapTypeInner {
    enum_accessors! {
        e
        (ConcreteFunc(CanonicalizedTypeIndex) is_concrete_func get_concrete_func unwrap_concrete_func *e)
        (ConcreteArray(CanonicalizedTypeIndex) is_concrete_array get_concrete_array unwrap_concrete_array *e)
        (ConcreteStruct(CanonicalizedTypeIndex) is_concrete_struct get_concrete_struct unwrap_concrete_struct *e)
    }
}

/// A concrete, user-defined (or host-defined) Wasm type.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmSubType {
    /// Whether this type is forbidden from being the supertype of any other
    /// type.
    pub is_final: bool,

    /// This type's supertype, if any.
    pub supertype: Option<CanonicalizedTypeIndex>,

    /// The array, function, or struct that is defined.
    pub composite_type: WasmCompositeType,
}

impl WasmSubType {
    #[inline]
    pub fn is_func(&self) -> bool {
        self.composite_type.is_func()
    }

    #[inline]
    pub fn as_func(&self) -> Option<&WasmFuncType> {
        self.composite_type.as_func()
    }

    #[inline]
    pub fn unwrap_func(&self) -> &WasmFuncType {
        self.composite_type.unwrap_func()
    }

    #[inline]
    pub fn is_array(&self) -> bool {
        self.composite_type.is_array()
    }

    #[inline]
    pub fn as_array(&self) -> Option<&WasmArrayType> {
        self.composite_type.as_array()
    }

    #[inline]
    pub fn unwrap_array(&self) -> &WasmArrayType {
        self.composite_type.unwrap_array()
    }

    #[inline]
    pub fn is_struct(&self) -> bool {
        self.composite_type.is_struct()
    }

    #[inline]
    pub fn as_struct(&self) -> Option<&WasmStructType> {
        self.composite_type.as_struct()
    }

    #[inline]
    pub fn unwrap_struct(&self) -> &WasmStructType {
        self.composite_type.unwrap_struct()
    }
}

impl fmt::Display for WasmSubType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_final && self.supertype.is_none() {
            fmt::Display::fmt(&self.composite_type, f)
        } else {
            write!(f, "(sub")?;
            if self.is_final {
                write!(f, " final")?;
            }
            if let Some(sup) = self.supertype {
                write!(f, " {sup:?}")?;
            }
            write!(f, " {})", self.composite_type)
        }
    }
}

impl TypeTrace for WasmSubType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        if let Some(sup) = self.supertype {
            func(sup)?;
        }
        self.composite_type.trace(func)
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        if let Some(sup) = self.supertype.as_mut() {
            func(sup)?;
        }
        self.composite_type.trace_mut(func)
    }
}

/// A function, array, or struct type.
///
/// Introduced by the GC proposal.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmCompositeType {
    pub inner: WasmCompositeTypeInner,
    /// Is the composite type shared? This is part of the
    /// shared-everything-threads proposal.
    pub shared: bool,
}

impl WasmCompositeType {
    pub(crate) fn new_func(shared: bool, ty: WasmFuncType) -> WasmCompositeType {
        Self {
            shared,
            inner: WasmCompositeTypeInner::Func(ty),
        }
    }
    pub(crate) fn new_array(shared: bool, ty: WasmArrayType) -> WasmCompositeType {
        Self {
            shared,
            inner: WasmCompositeTypeInner::Array(ty),
        }
    }
    pub(crate) fn new_struct(shared: bool, ty: WasmStructType) -> WasmCompositeType {
        Self {
            shared,
            inner: WasmCompositeTypeInner::Struct(ty),
        }
    }
    #[inline]
    pub fn is_func(&self) -> bool {
        self.inner.is_func()
    }

    #[inline]
    pub fn as_func(&self) -> Option<&WasmFuncType> {
        self.inner.as_func()
    }

    #[inline]
    pub fn unwrap_func(&self) -> &WasmFuncType {
        self.inner.unwrap_func()
    }

    #[inline]
    pub fn is_array(&self) -> bool {
        self.inner.is_array()
    }

    #[inline]
    pub fn as_array(&self) -> Option<&WasmArrayType> {
        self.inner.as_array()
    }

    #[inline]
    pub fn unwrap_array(&self) -> &WasmArrayType {
        self.inner.unwrap_array()
    }

    #[inline]
    pub fn is_struct(&self) -> bool {
        self.inner.is_struct()
    }

    #[inline]
    pub fn as_struct(&self) -> Option<&WasmStructType> {
        self.inner.as_struct()
    }

    #[inline]
    pub fn unwrap_struct(&self) -> &WasmStructType {
        self.inner.unwrap_struct()
    }
}

impl fmt::Display for WasmCompositeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.shared {
            write!(f, "shared ")?;
        }
        match &self.inner {
            WasmCompositeTypeInner::Func(ty) => fmt::Display::fmt(ty, f),
            WasmCompositeTypeInner::Array(ty) => fmt::Display::fmt(ty, f),
            WasmCompositeTypeInner::Struct(ty) => fmt::Display::fmt(ty, f),
        }
    }
}

impl TypeTrace for WasmCompositeType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        match self.inner {
            WasmCompositeTypeInner::Func(ref inner) => inner.trace(func),
            WasmCompositeTypeInner::Array(ref inner) => inner.trace(func),
            WasmCompositeTypeInner::Struct(ref inner) => inner.trace(func),
        }
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        match self.inner {
            WasmCompositeTypeInner::Func(ref mut inner) => inner.trace_mut(func),
            WasmCompositeTypeInner::Array(ref mut inner) => inner.trace_mut(func),
            WasmCompositeTypeInner::Struct(ref mut inner) => inner.trace_mut(func),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WasmCompositeTypeInner {
    /// The type is a regular function.
    Func(WasmFuncType),
    /// The type is a GC-proposal array.
    Array(WasmArrayType),
    /// The type is a GC-proposal struct.
    Struct(WasmStructType),
}

impl WasmCompositeTypeInner {
    enum_accessors! {
        c
        (Func(&WasmFuncType) is_func as_func unwrap_func c)
        (Array(&WasmArrayType) is_array as_array unwrap_array c)
        (Struct(&WasmStructType) is_struct as_struct unwrap_struct c)
    }
}

/// A WebAssembly function type.
///
/// This is the equivalent of `wasmparser::FuncType`.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmFuncType {
    pub params: Box<[WasmValType]>,
    pub results: Box<[WasmValType]>,
}

impl fmt::Display for WasmFuncType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(func")?;
        if !self.params.is_empty() {
            write!(f, " (param")?;
            for p in &self.params {
                write!(f, " {p}")?;
            }
            write!(f, ")")?;
        }
        if !self.results.is_empty() {
            write!(f, " (result")?;
            for r in &self.results {
                write!(f, " {r}")?;
            }
            write!(f, ")")?;
        }
        write!(f, ")")
    }
}

impl TypeTrace for WasmFuncType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        for ty in &self.params {
            ty.trace(func)?;
        }
        for ty in &self.results {
            ty.trace(func)?;
        }
        Ok(())
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        for ty in &mut self.params {
            ty.trace_mut(func)?;
        }
        for ty in &mut self.results {
            ty.trace_mut(func)?;
        }
        Ok(())
    }
}

/// A WebAssembly GC-proposal Array type.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmArrayType(pub WasmFieldType);

impl fmt::Display for WasmArrayType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(array {})", self.0)
    }
}

impl TypeTrace for WasmArrayType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        self.0.trace(func)
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        self.0.trace_mut(func)
    }
}

/// A WebAssembly GC-proposal struct type.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmStructType {
    pub fields: Box<[WasmFieldType]>,
}

impl fmt::Display for WasmStructType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(struct")?;
        for ty in &self.fields {
            write!(f, " {ty}")?;
        }
        write!(f, ")")
    }
}

impl TypeTrace for WasmStructType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        for field in &self.fields {
            field.trace(func)?;
        }
        Ok(())
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        for field in &mut self.fields {
            field.trace_mut(func)?;
        }
        Ok(())
    }
}

/// The type of struct field or array element.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmFieldType {
    /// Whether this field can be mutated or not.
    pub mutable: bool,
    /// The field's element type.
    pub element_type: WasmStorageType,
}

impl fmt::Display for WasmFieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mutable {
            write!(f, "(mut {})", self.element_type)
        } else {
            fmt::Display::fmt(&self.element_type, f)
        }
    }
}

impl TypeTrace for WasmFieldType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        self.element_type.trace(func)
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        self.element_type.trace_mut(func)
    }
}

/// A WebAssembly GC-proposal storage type for Array and Struct fields.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WasmStorageType {
    /// The storage type is i8.
    I8,
    /// The storage type is i16.
    I16,
    /// The storage type is a value type.
    Val(WasmValType),
}

impl fmt::Display for WasmStorageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WasmStorageType::I8 => write!(f, "i8"),
            WasmStorageType::I16 => write!(f, "i16"),
            WasmStorageType::Val(v) => fmt::Display::fmt(v, f),
        }
    }
}

impl TypeTrace for WasmStorageType {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        if let Self::Val(val) = self {
            val.trace(func)
        } else {
            Ok(())
        }
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        if let Self::Val(val) = self {
            val.trace_mut(func)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub enum EntityType {
    Function(CanonicalizedTypeIndex),
    Table(TableDesc),
    Memory(MemoryDesc),
    Global(GlobalDesc),
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct WasmRecGroup(pub(crate) Box<[WasmSubType]>);

impl TypeTrace for WasmRecGroup {
    fn trace<F, E>(&self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(CanonicalizedTypeIndex) -> Result<(), E>,
    {
        for ty in &self.0 {
            ty.trace(func)?;
        }
        Ok(())
    }

    fn trace_mut<F, E>(&mut self, func: &mut F) -> Result<(), E>
    where
        F: FnMut(&mut CanonicalizedTypeIndex) -> Result<(), E>,
    {
        for ty in &mut self.0 {
            ty.trace_mut(func)?;
        }
        Ok(())
    }
}
