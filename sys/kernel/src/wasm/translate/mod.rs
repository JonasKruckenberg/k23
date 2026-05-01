// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod const_expr;
mod module_translator;
mod module_types;
mod type_convert;
mod types;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use anyhow::Context;
pub use const_expr::{ConstExpr, ConstOp};
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::{EntityRef, EntitySet, PrimaryMap};
use hashbrown::HashMap;
pub use module_translator::ModuleTranslator;
pub use module_types::ModuleTypes;
pub use type_convert::WasmparserTypeConverter;
pub use types::{
    EntityType, WasmCompositeType, WasmCompositeTypeInner, WasmFieldType, WasmFuncType,
    WasmHeapTopType, WasmHeapType, WasmHeapTypeInner, WasmRecGroup, WasmRefType, WasmStorageType,
    WasmSubType, WasmValType,
};
use wasmparser::WasmFeatures;
use wasmparser::collections::IndexMap;

use crate::wasm::indices::{
    CanonicalizedTypeIndex, DataIndex, DefinedFuncIndex, DefinedGlobalIndex, DefinedMemoryIndex,
    DefinedTableIndex, DefinedTagIndex, ElemIndex, EntityIndex, FieldIndex, FuncIndex,
    FuncRefIndex, GlobalIndex, LabelIndex, LocalIndex, MemoryIndex, ModuleInternedTypeIndex,
    OwnedMemoryIndex, TableIndex, TagIndex, TypeIndex,
};
use crate::wasm::{DEFAULT_OFFSET_GUARD_SIZE, WASM32_MAX_SIZE};

#[derive(Debug)]
pub struct ModuleTranslation<'data> {
    /// The translated module.
    pub module: TranslatedModule,
    /// Information about the module's functions.
    pub function_bodies: PrimaryMap<DefinedFuncIndex, FunctionBodyData<'data>>,
    /// DWARF and other debug information parsed from the module.
    pub debug_info: DebugInfo<'data>,
    /// Required WASM features (proposals etc.) as self-reported by the module through the `wasm-features` custom section.
    /// Later on this could be used to determine which compiler/runtime features to enable, but
    /// for now we just use it to assert compatibility.
    pub required_features: WasmFeatures,
}

impl Default for ModuleTranslation<'_> {
    fn default() -> Self {
        Self {
            module: TranslatedModule::default(),
            function_bodies: PrimaryMap::default(),
            debug_info: DebugInfo::default(),
            required_features: WasmFeatures::empty(),
        }
    }
}

/// A translated WebAssembly module.
#[derive(Debug, Default)]
pub struct TranslatedModule {
    /// The name of this wasm module, if found,
    pub name: Option<String>,
    /// The types declared in this module.
    pub types: PrimaryMap<TypeIndex, ModuleInternedTypeIndex>,

    /// The functions declared in this module. Note that this only contains the
    /// function's signature index, not the actual function body. For that, see `Translation.function_bodies`.
    pub functions: PrimaryMap<FuncIndex, Function>,
    /// The tables declared in this module.
    pub tables: PrimaryMap<TableIndex, Table>,
    /// The memories declared in this module.
    pub memories: PrimaryMap<MemoryIndex, Memory>,
    /// The globals declared in this module.
    pub globals: PrimaryMap<GlobalIndex, Global>,
    /// The tags declared in this module.
    pub tags: PrimaryMap<TagIndex, Tag>,

    /// The index of the start function if defined.
    /// This function will be called during module initialization.
    pub start: Option<FuncIndex>,
    /// Imports declared in this module.
    pub imports: Vec<Import>,
    /// Exports declared in this module.
    pub exports: IndexMap<String, EntityIndex>,

    /// Initialization expressions for globals defined in this module.
    pub global_initializers: PrimaryMap<DefinedGlobalIndex, ConstExpr>,
    /// Initializers for tables defined or imported in this module.
    pub table_initializers: TableInitializers,
    /// Initializers for memories defined or imported in this module.
    pub memory_initializers: Vec<MemoryInitializer>,

    /// Passive table initializers that can be access by `table.init` instructions.
    pub passive_table_initializers: HashMap<ElemIndex, TableSegmentElements>,
    /// Passive memory initializers that can be access by `memory.init` instructions.
    pub passive_memory_initializers: HashMap<DataIndex, Vec<u8>>,
    /// `ElemIndex`es of active table initializers that should be treated as "dropped" at runtime.
    pub active_table_initializers: EntitySet<ElemIndex>,
    /// `DataIndex`es of active memory initializers that should be treated as "dropped" at runtime.
    pub active_memory_initializers: EntitySet<DataIndex>,
    /// The number of imported functions. The first `num_imported_functions` functions in the `functions`
    /// table are imported functions.
    pub num_imported_functions: u32,
    /// The number of imported tables. The first `num_imported_tables` tables in the `tables` table are imported tables.
    pub num_imported_tables: u32,
    /// The number of imported memories. The first `num_imported_memories` memories in the `memories` table are imported memories.
    pub num_imported_memories: u32,
    /// The number of imported globals. The first `num_imported_globals` globals in the `globals` table are imported globals.
    pub num_imported_globals: u32,
    /// The number of imported tags. The first `num_imported_tags` globals in the `tags` table are imported tags.
    pub num_imported_tags: u32,
    /// The number of imported functions. This is used to compile host->wasm trampolines later.
    pub num_escaped_functions: u32,
}

impl TranslatedModule {
    #[inline]
    pub fn func_index(&self, index: DefinedFuncIndex) -> FuncIndex {
        FuncIndex::from_u32(self.num_imported_functions + index.as_u32())
    }

    #[inline]
    pub fn defined_func_index(&self, index: FuncIndex) -> Option<DefinedFuncIndex> {
        if self.is_imported_func(index) {
            None
        } else {
            Some(DefinedFuncIndex::from_u32(
                index.as_u32() - self.num_imported_functions,
            ))
        }
    }

    #[inline]
    pub fn is_imported_func(&self, index: FuncIndex) -> bool {
        index.as_u32() < self.num_imported_functions
    }

    #[inline]
    pub fn table_index(&self, index: DefinedTableIndex) -> TableIndex {
        TableIndex::from_u32(self.num_imported_tables + index.as_u32())
    }

    #[inline]
    pub fn defined_table_index(&self, index: TableIndex) -> Option<DefinedTableIndex> {
        if self.is_imported_table(index) {
            None
        } else {
            Some(DefinedTableIndex::from_u32(
                index.as_u32() - self.num_imported_tables,
            ))
        }
    }

    #[inline]
    pub fn is_imported_table(&self, index: TableIndex) -> bool {
        index.as_u32() < self.num_imported_tables
    }

    #[inline]
    pub fn defined_memory_index(&self, index: MemoryIndex) -> Option<DefinedMemoryIndex> {
        if self.is_imported_memory(index) {
            None
        } else {
            Some(DefinedMemoryIndex::from_u32(
                index.as_u32() - self.num_imported_memories,
            ))
        }
    }

    #[inline]
    pub fn owned_memory_index(&self, memory: DefinedMemoryIndex) -> OwnedMemoryIndex {
        assert!(
            memory.index() < self.memories.len(),
            "non-shared memory must have an owned index"
        );

        // Once we know that the memory index is not greater than the number of
        // plans, we can iterate through the plans up to the memory index and
        // count how many are not shared (i.e., owned).
        let owned_memory_index = self
            .memories
            .iter()
            .skip(self.num_imported_memories as usize)
            .take(memory.index())
            .filter(|(_, mp)| !mp.shared)
            .count();
        OwnedMemoryIndex::new(owned_memory_index)
    }

    #[inline]
    pub fn is_imported_memory(&self, index: MemoryIndex) -> bool {
        index.as_u32() < self.num_imported_memories
    }

    #[inline]
    pub fn global_index(&self, index: DefinedGlobalIndex) -> GlobalIndex {
        GlobalIndex::from_u32(self.num_imported_globals + index.as_u32())
    }

    #[inline]
    pub fn defined_global_index(&self, index: GlobalIndex) -> Option<DefinedGlobalIndex> {
        if self.is_imported_global(index) {
            None
        } else {
            Some(DefinedGlobalIndex::from_u32(
                index.as_u32() - self.num_imported_globals,
            ))
        }
    }

    #[inline]
    pub fn is_imported_global(&self, index: GlobalIndex) -> bool {
        index.as_u32() < self.num_imported_globals
    }

    #[inline]
    pub fn tag_index(&self, index: DefinedTagIndex) -> TagIndex {
        TagIndex::from_u32(self.num_imported_tags + index.as_u32())
    }

    #[inline]
    pub fn defined_tag_index(&self, index: TagIndex) -> Option<DefinedTagIndex> {
        if self.is_imported_tag(index) {
            None
        } else {
            Some(DefinedTagIndex::from_u32(
                index.as_u32() - self.num_imported_tags,
            ))
        }
    }

    #[inline]
    pub fn is_imported_tag(&self, index: TagIndex) -> bool {
        index.as_u32() < self.num_imported_tags
    }

    pub fn num_imported_functions(&self) -> u32 {
        self.num_imported_functions
    }
    pub fn num_imported_tables(&self) -> u32 {
        self.num_imported_tables
    }
    pub fn num_imported_memories(&self) -> u32 {
        self.num_imported_memories
    }
    pub fn num_imported_globals(&self) -> u32 {
        self.num_imported_globals
    }
    pub fn num_defined_tables(&self) -> u32 {
        self.num_tables() - self.num_imported_tables
    }
    pub fn num_defined_memories(&self) -> u32 {
        self.num_memories() - self.num_imported_memories
    }
    pub fn num_defined_globals(&self) -> u32 {
        self.num_globals() - self.num_imported_globals
    }
    pub fn num_defined_tags(&self) -> u32 {
        self.num_tags() - self.num_imported_tags
    }
    pub fn num_functions(&self) -> u32 {
        u32::try_from(self.functions.len()).unwrap()
    }
    pub fn num_tables(&self) -> u32 {
        u32::try_from(self.tables.len()).unwrap()
    }
    pub fn num_memories(&self) -> u32 {
        u32::try_from(self.memories.len()).unwrap()
    }
    pub fn num_globals(&self) -> u32 {
        u32::try_from(self.globals.len()).unwrap()
    }
    pub fn num_tags(&self) -> u32 {
        u32::try_from(self.tags.len()).unwrap()
    }

    pub fn num_escaped_funcs(&self) -> u32 {
        self.num_escaped_functions
    }
}

#[derive(Debug)]
pub struct FunctionBodyData<'data> {
    pub body: wasmparser::FunctionBody<'data>,
    pub validator: wasmparser::FuncToValidate<wasmparser::ValidatorResources>,
}

#[derive(Debug)]
pub struct Function {
    /// The index of the function signature in the type section.
    pub signature: CanonicalizedTypeIndex,
    /// And index identifying this function "to the outside world"
    /// or the reserved value if the function isn't escaping from its module.
    pub func_ref: FuncRefIndex,
}

impl Function {
    pub fn is_escaping(&self) -> bool {
        !self.func_ref.is_reserved_value()
    }
}

/// The type that can be used to index into [Memory] and [Table].
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
#[allow(missing_docs, reason = "self-describing variants")]
pub enum IndexType {
    I32,
    I64,
}

/// The size range of resizeable storage associated with [Memory] types and [Table] types.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
#[allow(missing_docs, reason = "self-describing fields")]
pub struct Limits {
    pub min: u64,
    pub max: Option<u64>,
}

#[derive(Debug, Clone, Hash)]
pub struct Table {
    /// The table's element type.
    pub element_type: WasmRefType,
    pub index_type: IndexType,
    pub limits: Limits,
    /// Whether this table is shared indicating that it should be send-able across threads and the maximum field is always present for valid types.
    ///
    /// This is included the shared-everything-threads proposal.
    pub shared: bool,
}

impl Table {
    /// Creates a new `TablePlan` for the given `wasmparser::TableType`.
    pub fn from_wasmparser(
        ty: wasmparser::TableType,
        type_convert: &WasmparserTypeConverter,
    ) -> Self {
        Self {
            element_type: type_convert.convert_ref_type(ty.element_type),
            index_type: if ty.table64 {
                IndexType::I64
            } else {
                IndexType::I32
            },
            limits: Limits {
                min: ty.initial,
                max: ty.maximum,
            },
            shared: ty.shared,
        }
    }
}

#[derive(Debug, Clone, Hash)]
pub struct Memory {
    pub limits: Limits,
    pub index_type: IndexType,
    /// The size in bytes of the offset guard region.
    pub offset_guard_size: u64,
    /// The log2 of this memory's page size, in bytes.
    ///
    /// By default, the page size is 64KiB (0x10000; 2**16; 1<<16; 65536) but the
    /// custom-page-sizes proposal allows opting into a page size of `1`.
    pub page_size_log2: u8,
    /// Whether or not this is a “shared” memory, indicating that it should be send-able across threads and the maximum field is always present for valid types.
    ///
    /// This is part of the threads proposal in WebAssembly.
    pub shared: bool,
}

impl Memory {
    /// WebAssembly page sizes are 64KiB by default.
    pub const DEFAULT_PAGE_SIZE: u32 = 0x10000;

    /// WebAssembly page sizes are 64KiB (or `2**16`) by default.
    pub const DEFAULT_PAGE_SIZE_LOG2: u8 = {
        let log2 = 16;
        assert!(1 << log2 == Self::DEFAULT_PAGE_SIZE);
        log2
    };

    /// Creates a new `MemoryPlan` for the given `wasmparser::MemoryType`.
    pub fn from_wasmparser(ty: wasmparser::MemoryType) -> Self {
        Self {
            index_type: if ty.memory64 {
                IndexType::I64
            } else {
                IndexType::I32
            },
            limits: Limits {
                min: ty.initial,
                max: ty.maximum,
            },
            shared: ty.shared,
            page_size_log2: ty
                .page_size_log2
                .map_or(Self::DEFAULT_PAGE_SIZE_LOG2, |log2| {
                    u8::try_from(log2).unwrap()
                }),
            offset_guard_size: DEFAULT_OFFSET_GUARD_SIZE,
        }
    }

    /// Returns the minimum size, in bytes, that this memory must be.
    ///
    /// # Errors
    ///
    /// Returns an error if the calculation of the minimum size overflows the
    /// `u64` return type. This means that the memory can't be allocated but
    /// it's deferred to the caller to how to deal with that.
    pub fn minimum_byte_size(&self) -> crate::Result<u64> {
        self.limits
            .min
            .checked_mul(self.page_size())
            .context("size overflow")
    }

    /// Returns the maximum size, in bytes, that this memory is allowed to be.
    ///
    /// Note that the return value here is not an `Option` despite the maximum
    /// size of a linear memory being optional in wasm. If a maximum size
    /// is not present in the memory's type then a maximum size is selected for
    /// it. For example the maximum size of a 32-bit memory is `1<<32`. The
    /// maximum size of a 64-bit linear memory is chosen to be a value that
    /// won't ever be allowed at runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the calculation of the maximum size overflows the
    /// `u64` return type. This means that the memory can't be allocated but
    /// it's deferred to the caller to how to deal with that.
    pub fn maximum_byte_size(&self) -> crate::Result<u64> {
        if let Some(max) = self.limits.max {
            max.checked_mul(self.page_size()).context("size overflow")
        } else {
            let min = self.minimum_byte_size()?;
            Ok(min.max(self.max_size_based_on_index_type()))
        }
    }

    /// Get the size of this memory's pages, in bytes.
    pub fn page_size(&self) -> u64 {
        debug_assert!(
            self.page_size_log2 == 16 || self.page_size_log2 == 0,
            "invalid page_size_log2: {}; must be 16 or 0",
            self.page_size_log2
        );
        1 << self.page_size_log2
    }

    /// Returns the maximum size memory is allowed to be only based on the
    /// index type used by this memory.
    ///
    /// For example 32-bit linear memories return `1<<32` from this method.
    pub fn max_size_based_on_index_type(&self) -> u64 {
        match self.index_type {
            IndexType::I32 => WASM32_MAX_SIZE,
            IndexType::I64 => {
                // Note that the true maximum size of a 64-bit linear memory, in
                // bytes, cannot be represented in a `u64`. That would require a u65
                // to store `1<<64`. Despite that no system can actually allocate a
                // full 64-bit linear memory so this is instead emulated as "what if
                // the kernel fit in a single Wasm page of linear memory". Shouldn't
                // ever actually be possible but it provides a number to serve as an
                // effective maximum.
                0_u64 - self.page_size()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Global {
    /// The type of value stored in this global.
    pub content_type: WasmValType,
    /// Whether this global is mutable.
    pub mutable: bool,
    /// Whether this global is shared, indicating that it should be send-able across threads.
    pub shared: bool,
}

impl Global {
    pub fn from_wasmparser(
        ty: wasmparser::GlobalType,
        type_convert: &WasmparserTypeConverter,
    ) -> Self {
        Self {
            content_type: type_convert.convert_val_type(ty.content_type),
            mutable: ty.mutable,
            shared: ty.shared,
        }
    }
}

/// WebAssembly exception and control tag.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub struct Tag {
    /// The tag signature type.
    pub signature: CanonicalizedTypeIndex,
}

#[derive(Debug, Default)]
pub struct TableInitializers {
    /// The initial values for table elements.
    pub initial_values: PrimaryMap<DefinedTableIndex, TableInitialValue>,
    pub segments: Vec<TableSegment>,
}

/// The initial value of a table.
#[derive(Debug)]
pub enum TableInitialValue {
    /// The table is initialized with null references.
    RefNull,
    /// The table is initialized with the result of a const expression.
    ConstExpr(ConstExpr),
}

#[derive(Debug)]
pub struct TableSegment {
    /// The index of the table being initialized.
    pub table_index: TableIndex,
    /// The offset at which to start filling.
    pub offset: ConstExpr,
    /// The elements to a slice of the table with.
    pub elements: TableSegmentElements,
}

#[derive(Debug, Clone)]
pub enum TableSegmentElements {
    /// The elements are a list of function indices.
    Functions(Box<[FuncIndex]>),
    /// The elements are produced by a list of constant expressions.
    Expressions(Box<[ConstExpr]>),
}

impl TableSegmentElements {
    pub fn len(&self) -> u64 {
        match self {
            TableSegmentElements::Functions(f) => f.len() as u64,
            TableSegmentElements::Expressions(e) => e.len() as u64,
        }
    }
}

#[derive(Debug)]
pub struct MemoryInitializer {
    /// The index of the memory being initialized.
    pub memory_index: MemoryIndex,
    /// The offset at which to start filling.
    pub offset: ConstExpr,
    /// The data to fill the memory with.
    /// This is an index into the `Translation.data` array.
    pub data: Vec<u8>,
}

/// A WebAssembly import.
#[derive(Debug)]
pub struct Import {
    /// The module or namespace being imported.
    pub module: String,
    /// The name of the item being imported.
    pub name: String,
    /// Where the imported entity will be placed, this also holds the type of the import.
    pub ty: EntityType,
}

#[derive(Debug, Default)]
pub struct DebugInfo<'wasm> {
    /// The names of various entities in the module.
    pub names: Names<'wasm>,
    /// Information about tools involved in the creation of the WASM module.
    pub producers: Producers<'wasm>,
    /// The offset of the code section in the original wasm file, used to calculate lookup values into the DWARF.
    pub code_section_offset: u64,
    pub dwarf: gimli::Dwarf<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_loc: gimli::DebugLoc<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_loclists: gimli::DebugLocLists<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_ranges: gimli::DebugRanges<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_rnglists: gimli::DebugRngLists<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_cu_index: gimli::DebugCuIndex<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_tu_index: gimli::DebugTuIndex<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
}

#[derive(Debug, Default)]
pub struct Names<'wasm> {
    pub funcs: HashMap<FuncIndex, &'wasm str>,
    pub locals: HashMap<FuncIndex, HashMap<LocalIndex, &'wasm str>>,
    pub globals: HashMap<GlobalIndex, &'wasm str>,
    pub data: HashMap<DataIndex, &'wasm str>,
    pub labels: HashMap<FuncIndex, HashMap<LabelIndex, &'wasm str>>,
    pub types: HashMap<TypeIndex, &'wasm str>,
    pub tables: HashMap<TableIndex, &'wasm str>,
    pub memories: HashMap<MemoryIndex, &'wasm str>,
    pub elements: HashMap<ElemIndex, &'wasm str>,
    pub fields: HashMap<FuncIndex, HashMap<FieldIndex, &'wasm str>>,
    pub tags: HashMap<TagIndex, &'wasm str>,
}

#[derive(Debug, Default)]
pub struct Producers<'wasm> {
    pub language: Vec<ProducersLanguageField<'wasm>>,
    pub processed_by: Vec<ProducersToolField<'wasm>>,
    pub sdk: Vec<ProducersSdkField<'wasm>>,
}

#[derive(Debug)]
pub struct ProducersLanguageField<'wasm> {
    pub name: ProducersLanguage<'wasm>,
    pub version: &'wasm str,
}

#[derive(Debug)]
pub enum ProducersLanguage<'wasm> {
    Wat,
    C,
    Cpp,
    Rust,
    JavaScript,
    Other(&'wasm str),
}

#[derive(Debug)]
pub struct ProducersToolField<'wasm> {
    pub name: ProducersTool<'wasm>,
    pub version: &'wasm str,
}

#[derive(Debug)]
pub enum ProducersTool<'wasm> {
    Wabt,
    Llvm,
    Clang,
    Lld,
    Binaryen,
    Rustc,
    WasmBindgen,
    WasmPack,
    Webassemblyjs,
    WasmSnip,
    Javy,
    Other(&'wasm str),
}

#[derive(Debug)]
pub struct ProducersSdkField<'wasm> {
    pub name: ProducersSdk<'wasm>,
    pub version: &'wasm str,
}

#[derive(Debug)]
pub enum ProducersSdk<'wasm> {
    Emscripten,
    Webpack,
    Other(&'wasm str),
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::wasm::translate::module_translator::ModuleTranslator;
//     use alloc::vec::Vec;
//     use wasmparser::Validator;
//
//     #[test_log::test]
//     fn simple_types() {
//         let wat = r#"(module
//             (type $f1 (func (param i32) (result i32)))
//         )"#;
//
//         let mut validator = Validator::new();
//
//         let wasm = wat::parse_str(wat).unwrap();
//         let (translation, types) = ModuleTranslator::new(&mut validator)
//             .translate(&wasm)
//             .unwrap();
//
//         let wasm_types = types.wasm_types().collect::<Vec<_>>();
//         assert_eq!(wasm_types.len(), 1);
//         assert!(wasm_types[0].1.is_func());
//         let f = wasm_types[0].1.unwrap_func();
//         assert_eq!(f.params.len(), 1);
//         assert_eq!(f.results.len(), 1);
//         assert_eq!(f.params[0], WasmValType::I32);
//         assert_eq!(f.results[0], WasmValType::I32);
//
//         assert_eq!(translation.module.types.len(), 1);
//     }
//
//     #[test_log::test]
//     fn simple_rec_group() {
//         let wat = r#"(module
//           (rec (type $f1 (func)) (type (struct (field (ref $f1)))))
//         )"#;
//
//         let mut validator = Validator::new();
//
//         let wasm = wat::parse_str(wat).unwrap();
//         let (_, types) = ModuleTranslator::new(&mut validator)
//             .translate(&wasm)
//             .unwrap();
//
//         for (idx, ty) in types.wasm_types() {
//             tracing::debug!("{idx:?} => {ty}")
//         }
//     }
// }
