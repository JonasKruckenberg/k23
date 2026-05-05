// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use core::fmt;

use crate::wasm::indices::CanonicalizedTypeIndex;
use crate::wasm::translate::{Global, Memory, Table};
use crate::wasm::type_registry::TypeTrace;
use crate::wasm::utils::enum_accessors;

/// Represents the types of values in a WebAssembly module.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
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

    fn trampoline_type(&self) -> Self {
        match self {
            WasmValType::Ref(r) => {
                let inner = match r.heap_type.top().0 {
                    WasmHeapTopType::Func => WasmHeapTypeInner::Func,
                    WasmHeapTopType::Extern => WasmHeapTypeInner::Extern,
                    WasmHeapTopType::Any => WasmHeapTypeInner::Any,
                    WasmHeapTopType::Exn => WasmHeapTypeInner::Exn,
                    WasmHeapTopType::Cont => WasmHeapTypeInner::Cont,
                };

                WasmValType::Ref(WasmRefType {
                    nullable: true,
                    heap_type: WasmHeapType {
                        shared: r.heap_type.top().1,
                        inner,
                    },
                })
            }
            WasmValType::I32
            | WasmValType::I64
            | WasmValType::F32
            | WasmValType::F64
            | WasmValType::V128 => *self,
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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
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

pub enum WasmHeapTopType {
    Func,
    Extern,
    Any,
    Exn,
    Cont,
}

pub enum WasmHeapBottomType {
    NoFunc,
    NoExtern,
    None,
    NoExn,
    NoCont,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
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
        match self.top().0 {
            // All `t <: (ref null any)` and `t <: (ref null extern)` are
            // represented as `VMGcRef`s.
            WasmHeapTopType::Any | WasmHeapTopType::Extern => true,
            // All others are not.
            _ => false,
        }
    }

    /// Get the top type of this heap type's type hierarchy.
    ///
    /// The returned heap type is a supertype of all types in this heap type's
    /// type hierarchy.
    pub fn top(&self) -> (WasmHeapTopType, bool) {
        let ty = match self.inner {
            WasmHeapTypeInner::Func
            | WasmHeapTypeInner::ConcreteFunc(_)
            | WasmHeapTypeInner::NoFunc => WasmHeapTopType::Func,

            WasmHeapTypeInner::Extern | WasmHeapTypeInner::NoExtern => WasmHeapTopType::Extern,

            WasmHeapTypeInner::Any
            | WasmHeapTypeInner::Eq
            | WasmHeapTypeInner::I31
            | WasmHeapTypeInner::Array
            | WasmHeapTypeInner::ConcreteArray(_)
            | WasmHeapTypeInner::Struct
            | WasmHeapTypeInner::ConcreteStruct(_)
            | WasmHeapTypeInner::None => WasmHeapTopType::Any,

            WasmHeapTypeInner::Exn | WasmHeapTypeInner::NoExn => WasmHeapTopType::Exn,
            WasmHeapTypeInner::Cont
            | WasmHeapTypeInner::ConcreteCont(_)
            | WasmHeapTypeInner::NoCont => WasmHeapTopType::Cont,
        };

        (ty, self.shared)
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
    pub fn bottom(&self) -> (WasmHeapBottomType, bool) {
        let ty = match self.inner {
            WasmHeapTypeInner::Extern | WasmHeapTypeInner::NoExtern => WasmHeapBottomType::NoExtern,

            WasmHeapTypeInner::Func
            | WasmHeapTypeInner::ConcreteFunc(_)
            | WasmHeapTypeInner::NoFunc => WasmHeapBottomType::NoFunc,

            WasmHeapTypeInner::Any
            | WasmHeapTypeInner::Eq
            | WasmHeapTypeInner::I31
            | WasmHeapTypeInner::Array
            | WasmHeapTypeInner::ConcreteArray(_)
            | WasmHeapTypeInner::Struct
            | WasmHeapTypeInner::ConcreteStruct(_)
            | WasmHeapTypeInner::None => WasmHeapBottomType::None,

            WasmHeapTypeInner::Exn | WasmHeapTypeInner::NoExn => WasmHeapBottomType::NoExn,
            WasmHeapTypeInner::Cont
            | WasmHeapTypeInner::ConcreteCont(_)
            | WasmHeapTypeInner::NoCont => WasmHeapBottomType::NoCont,
        };

        (ty, self.shared)
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
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
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
    ConcreteCont(CanonicalizedTypeIndex),
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
            WasmHeapTypeInner::ConcreteCont(i) => write!(f, "cont {i:?}"),
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

impl WasmFuncType {
    /// Is this function type compatible with trampoline usage?
    pub fn is_trampoline_type(&self) -> bool {
        self.params.iter().all(|p| *p == p.trampoline_type())
            && self.results.iter().all(|r| *r == r.trampoline_type())
    }

    /// Get the version of this function type that is suitable for usage as a
    /// trampoline.
    ///
    /// If this function is suitable for trampoline usage as-is, then a borrowed
    /// `Cow` is returned. If it must be tweaked for trampoline usage, then an
    /// owned `Cow` is returned.
    ///
    /// ## What is a trampoline type?
    ///
    /// All reference types in parameters and results are mapped to their
    /// nullable top type, e.g. `(ref $my_struct_type)` becomes `(ref null
    /// any)`.
    ///
    /// This allows us to share trampolines between functions whose signatures
    /// both map to the same trampoline type. It also allows the host to satisfy
    /// a Wasm module's function import of type `S` with a function of type `T`
    /// where `T <: S`, even when the Wasm module never defines the type `T`
    /// (and might never even be able to!)
    ///
    /// The flip side is that this adds a constraint to our trampolines: they
    /// can only pass references around (e.g. move a reference from one calling
    /// convention's location to another's) and may not actually inspect the
    /// references themselves (unless the trampolines start doing explicit,
    /// fallible downcasts, but if we ever need that, then we might want to
    /// redesign this stuff).
    pub fn trampoline_type(&self) -> Cow<'_, Self> {
        if self.is_trampoline_type() {
            return Cow::Borrowed(self);
        }

        Cow::Owned(Self {
            params: self.params.iter().map(|p| p.trampoline_type()).collect(),
            results: self.results.iter().map(|r| r.trampoline_type()).collect(),
        })
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
    Table(Table),
    Memory(Memory),
    Global(Global),
    Tag(CanonicalizedTypeIndex),
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
