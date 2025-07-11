use crate::Engine;
use crate::indices::{CanonicalizedTypeIndex, VMSharedTypeIndex};
use crate::type_registry::RegisteredType;
use crate::wasm::{
    WasmEntityType, WasmFieldType, WasmHeapType, WasmHeapTypeInner, WasmMemoryType, WasmRefType,
    WasmStorageType, WasmTableType, WasmValType,
};
use core::fmt;

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

#[derive(Clone, Hash)]
pub enum ValType {
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

#[derive(Clone, Hash)]
pub struct RefType {
    nullable: bool,
    heap_type: HeapType,
}

#[derive(Debug, Clone, Hash)]
pub struct HeapType {
    shared: bool,
    inner: HeapTypeInner,
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

/// The type of a `struct` field or an `array`'s elements.
///
/// This is a pair of both the field's storage type and its mutability
/// (i.e. whether the field can be updated or not).
#[derive(Clone, Hash)]
pub struct FieldType {
    mutability: Mutability,
    element_type: StorageType,
}

#[derive(Debug, Clone, Hash)]
pub struct StructType {
    registered_type: RegisteredType,
}

#[derive(Debug, Clone, Hash)]
pub struct ArrayType {
    registered_type: RegisteredType,
}

#[derive(Debug, Clone, Hash)]
pub struct FuncType {
    registered_type: RegisteredType,
}

/// A descriptor for a table in a WebAssembly module.
///
/// Tables are contiguous chunks of a specific element, typically a `funcref` or
/// an `externref`. The most common use for tables is a function table through
/// which `call_indirect` can invoke other functions.
#[derive(Debug, Clone, Hash)]
pub struct TableType {
    element: RefType,
    ty: WasmTableType,
}

/// A descriptor for a WebAssembly memory type.
///
/// Memories are described in units of pages (64KB) and represent contiguous
/// chunks of addressable memory.
#[derive(Debug, Clone, Hash)]
pub struct MemoryType {
    ty: WasmMemoryType,
}

/// A WebAssembly global descriptor.
///
/// This type describes an instance of a global in a WebAssembly module. Globals
/// are local to an [`Instance`](crate::wasm::Instance) and are either immutable or
/// mutable.
#[derive(Debug, Clone, Hash)]
pub struct GlobalType {
    content: ValType,
    mutability: Mutability,
}

/// A descriptor for a tag in a WebAssembly module.
///
/// This type describes an instance of a tag in a WebAssembly
/// module. Tags are local to an [`Instance`](crate::wasm::Instance).
#[derive(Debug, Clone, Hash)]
pub struct TagType {
    ty: FuncType,
}

/// A descriptor for an imported value into a wasm module.
///
/// This type is primarily accessed from the
/// [`Module::imports`](crate::wasm::TranslatedModule::imports) API. Each [`ImportType`]
/// describes an import into the wasm module with the module/name that it's
/// imported from as well as the type of item that's being imported.
#[derive(Clone)]
pub struct ImportType<'module> {
    /// The module of the import.
    module: &'module str,
    /// The field of the import.
    name: &'module str,
    /// The type of the import.
    ty: WasmEntityType,
    // types: &'module ModuleTypes,
    engine: &'module Engine,
}

/// A descriptor for an exported WebAssembly value.
///
/// This type is primarily accessed from the
/// [`Module::exports`](crate::wasm::TranslatedModule::exports) accessor and describes what
/// names are exported from a wasm module and the type of the item that is
/// exported.
#[derive(Clone)]
pub struct ExportType<'module> {
    /// The name of the export.
    name: &'module str,
    /// The type of the export.
    ty: WasmEntityType,
    // types: &'module ModuleTypes,
    engine: &'module Engine,
}

// === impl Mutability ===

impl Mutability {
    /// Is this constant?
    #[inline]
    pub const fn is_const(self) -> bool {
        matches!(self, Mutability::Const)
    }

    /// Is this variable?
    #[inline]
    pub const fn is_var(self) -> bool {
        matches!(self, Mutability::Var)
    }
}

// === impl Finality ===

impl Finality {
    /// Is this final?
    #[inline]
    pub const fn is_final(self) -> bool {
        matches!(self, Finality::Final)
    }

    /// Is this non-final?
    #[inline]
    pub const fn is_non_final(self) -> bool {
        matches!(self, Finality::NonFinal)
    }
}

// === impl ValType ===

impl ValType {
    fn from_wasm_type(engine: &Engine, ty: &WasmValType) -> Self {
        match ty {
            WasmValType::I32 => ValType::I32,
            WasmValType::I64 => ValType::I64,
            WasmValType::F32 => ValType::F32,
            WasmValType::F64 => ValType::F64,
            WasmValType::V128 => ValType::V128,
            WasmValType::Ref(r) => ValType::Ref(RefType::from_wasm_type(engine, r)),
        }
    }
}

impl fmt::Debug for ValType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for ValType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ValType::I32 => write!(f, "i32"),
            ValType::I64 => write!(f, "i64"),
            ValType::F32 => write!(f, "f32"),
            ValType::F64 => write!(f, "f64"),
            ValType::V128 => write!(f, "v128"),
            ValType::Ref(r) => fmt::Display::fmt(r, f),
        }
    }
}

// === impl RefType ===

impl RefType {
    pub const fn new(nullable: bool, heap_type: HeapType) -> Self {
        Self {
            nullable,
            heap_type,
        }
    }

    /// Can this type of reference be null?
    #[inline]
    pub const fn is_nullable(&self) -> bool {
        self.nullable
    }

    /// The heap type that this is a reference to.
    #[inline]
    pub const fn heap_type(&self) -> &HeapType {
        &self.heap_type
    }

    fn from_wasm_type(engine: &Engine, ty: &WasmRefType) -> Self {
        Self {
            nullable: ty.nullable,
            heap_type: HeapType::from_wasm_type(engine, ty.heap_type),
        }
    }
}

impl fmt::Debug for RefType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
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

// === impl HeapType ===

impl HeapType {
    pub const fn new(shared: bool, inner: HeapTypeInner) -> Self {
        Self { shared, inner }
    }

    #[inline]
    pub const fn shared(&self) -> bool {
        self.shared
    }

    #[inline]
    pub const fn inner(&self) -> &HeapTypeInner {
        &self.inner
    }

    /// Get the top type of this heap type's type hierarchy.
    ///
    /// The returned heap type is a supertype of all types in this heap type's
    /// type hierarchy.
    #[inline]
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

        Self::new(self.shared, inner)
    }

    /// Is this the top type within its type hierarchy?
    #[inline]
    pub fn is_top(&self) -> bool {
        matches!(
            self.inner,
            HeapTypeInner::Any
                | HeapTypeInner::Extern
                | HeapTypeInner::Func
                | HeapTypeInner::Cont
                | HeapTypeInner::Exn
        )
    }

    /// Get the bottom type of this heap type's type hierarchy.
    ///
    /// The returned heap type is a subtype of all types in this heap type's
    /// type hierarchy.
    #[inline]
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

            HeapTypeInner::Exn | HeapTypeInner::NoExn => HeapTypeInner::Exn,
            HeapTypeInner::Cont | HeapTypeInner::NoCont => HeapTypeInner::Cont,
        };
        Self::new(self.shared, inner)
    }

    /// Is this the bottom type within its type hierarchy?
    #[inline]
    pub fn is_bottom(&self) -> bool {
        matches!(
            self.inner,
            HeapTypeInner::None
                | HeapTypeInner::NoExtern
                | HeapTypeInner::NoFunc
                | HeapTypeInner::NoCont
                | HeapTypeInner::NoExn
        )
    }

    fn from_wasm_type(engine: &Engine, ty: WasmHeapType) -> Self {
        Self {
            shared: ty.shared,
            inner: match ty.inner {
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
                WasmHeapTypeInner::Exn => HeapTypeInner::Exn,
                WasmHeapTypeInner::NoExn => HeapTypeInner::NoExn,
                WasmHeapTypeInner::Cont => HeapTypeInner::Cont,
                WasmHeapTypeInner::ConcreteCont(_) => todo!(),
                WasmHeapTypeInner::NoCont => HeapTypeInner::NoCont,

                WasmHeapTypeInner::ConcreteFunc(CanonicalizedTypeIndex::Engine(idx)) => {
                    HeapTypeInner::ConcreteFunc(FuncType::from_shared_type_index(engine, idx))
                }
                WasmHeapTypeInner::ConcreteArray(CanonicalizedTypeIndex::Engine(idx)) => {
                    HeapTypeInner::ConcreteArray(ArrayType::from_shared_type_index(engine, idx))
                }
                WasmHeapTypeInner::ConcreteStruct(CanonicalizedTypeIndex::Engine(idx)) => {
                    HeapTypeInner::ConcreteStruct(StructType::from_shared_type_index(engine, idx))
                }
                WasmHeapTypeInner::ConcreteFunc(
                    CanonicalizedTypeIndex::Module(_) | CanonicalizedTypeIndex::RecGroup(_),
                )
                | WasmHeapTypeInner::ConcreteArray(
                    CanonicalizedTypeIndex::Module(_) | CanonicalizedTypeIndex::RecGroup(_),
                )
                | WasmHeapTypeInner::ConcreteStruct(
                    CanonicalizedTypeIndex::Module(_) | CanonicalizedTypeIndex::RecGroup(_),
                ) => {
                    panic!(
                        "HeapTypeInner::from_wasm_type on non-canonicalized-for-runtime-usage heap type"
                    )
                }
            },
        }
    }
}

impl fmt::Display for HeapType {
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

// === impl StorageType ===

impl StorageType {
    pub fn unpack(&self) -> &ValType {
        match self {
            StorageType::I8 | StorageType::I16 => &ValType::I32,
            StorageType::ValType(ty) => ty,
        }
    }

    fn from_wasm_type(engine: &Engine, ty: &WasmStorageType) -> Self {
        match ty {
            WasmStorageType::I8 => StorageType::I8,
            WasmStorageType::I16 => StorageType::I16,
            WasmStorageType::Val(v) => StorageType::ValType(ValType::from_wasm_type(engine, v)),
        }
    }
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

// === impl FieldType ===

impl FieldType {
    pub fn mutability(&self) -> Mutability {
        self.mutability
    }

    pub fn element_type(&self) -> &StorageType {
        &self.element_type
    }

    fn from_wasm_type(engine: &Engine, ty: &WasmFieldType) -> Self {
        Self {
            mutability: if ty.mutable {
                Mutability::Var
            } else {
                Mutability::Const
            },
            element_type: StorageType::from_wasm_type(engine, &ty.element_type),
        }
    }
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

// === impl StructType ===

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
        let engine = self.engine();

        self.registered_type
            .unwrap_struct()
            .fields
            .iter()
            .map(|wasm_ty| FieldType::from_wasm_type(engine, wasm_ty))
    }

    pub(crate) fn engine(&self) -> &Engine {
        self.registered_type.engine()
    }

    pub(crate) fn type_index(&self) -> VMSharedTypeIndex {
        self.registered_type.index()
    }

    pub(crate) fn from_shared_type_index(engine: &Engine, index: VMSharedTypeIndex) -> StructType {
        let ty = engine.type_registry().root(engine, index).expect(
            "VMSharedTypeIndex is not registered in the Engine! Wrong \
             engine? Didn't root the index somewhere?",
        );
        Self::from_registered_type(ty)
    }

    pub(crate) fn from_registered_type(registered_type: RegisteredType) -> Self {
        debug_assert!(registered_type.is_struct());
        Self { registered_type }
    }
}

// === impl ArrayType ===

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

    pub(crate) fn engine(&self) -> &Engine {
        self.registered_type.engine()
    }

    pub(crate) fn type_index(&self) -> VMSharedTypeIndex {
        self.registered_type.index()
    }

    pub(crate) fn from_shared_type_index(engine: &Engine, index: VMSharedTypeIndex) -> ArrayType {
        let ty = engine.type_registry().root(engine, index).expect(
            "VMSharedTypeIndex is not registered in the Engine! Wrong \
             engine? Didn't root the index somewhere?",
        );
        Self::from_registered_type(ty)
    }

    pub(crate) fn from_registered_type(registered_type: RegisteredType) -> Self {
        debug_assert!(registered_type.is_array());
        Self { registered_type }
    }
}

// === impl FuncType ===

impl fmt::Display for FuncType {
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
    pub fn param(&self, i: usize) -> Option<ValType> {
        let engine = self.engine();

        self.registered_type
            .unwrap_func()
            .params
            .get(i)
            .map(|ty| ValType::from_wasm_type(engine, ty))
    }

    pub fn params(&self) -> impl ExactSizeIterator<Item = ValType> + '_ {
        let engine = self.engine();

        self.registered_type
            .unwrap_func()
            .params
            .iter()
            .map(|ty| ValType::from_wasm_type(engine, ty))
    }

    pub fn result(&self, i: usize) -> Option<ValType> {
        let engine = self.engine();
        self.registered_type
            .unwrap_func()
            .results
            .get(i)
            .map(|ty| ValType::from_wasm_type(engine, ty))
    }

    pub fn results(&self) -> impl ExactSizeIterator<Item = ValType> + '_ {
        let engine = self.engine();

        self.registered_type
            .unwrap_func()
            .results
            .iter()
            .map(|ty| ValType::from_wasm_type(engine, ty))
    }

    pub(crate) fn type_index(&self) -> VMSharedTypeIndex {
        self.registered_type.index()
    }

    pub(crate) fn engine(&self) -> &Engine {
        self.registered_type.engine()
    }

    pub(crate) fn from_shared_type_index(engine: &Engine, index: VMSharedTypeIndex) -> FuncType {
        let ty = engine.type_registry().root(engine, index).expect(
            "VMSharedTypeIndex is not registered in the Engine! Wrong \
             engine? Didn't root the index somewhere?",
        );
        Self::from_registered_type(ty)
    }

    pub(crate) fn from_registered_type(registered_type: RegisteredType) -> Self {
        debug_assert!(registered_type.is_func());
        Self { registered_type }
    }
}
