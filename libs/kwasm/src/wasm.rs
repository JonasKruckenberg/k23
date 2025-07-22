// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod const_expr;
mod module_parser;
mod module_types;
mod type_convert;
mod types;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

pub use const_expr::{ConstExpr, ConstOp};
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::{EntitySet, PrimaryMap};
use hashbrown::HashMap;
pub use module_parser::ModuleParser;
pub use module_types::ModuleTypes;
pub use type_convert::WasmparserTypeConverter;
pub use types::{
    WasmArrayType, WasmCompositeType, WasmCompositeTypeInner, WasmEntityType, WasmFieldType,
    WasmFuncType, WasmGlobalType, WasmHeapType, WasmHeapTypeInner, WasmIndexType, WasmMemoryType,
    WasmRecGroup, WasmRefType, WasmStorageType, WasmStructType, WasmSubType, WasmTableType,
    WasmTagType, WasmValType,
};
pub use wasmparser::WasmFeatures;
use wasmparser::collections::IndexMap;

use crate::indices::{
    CanonicalizedTypeIndex, DataIndex, DefinedFuncIndex, DefinedGlobalIndex, DefinedMemoryIndex,
    DefinedTableIndex, DefinedTagIndex, ElemIndex, EntityIndex, FieldIndex, FuncIndex,
    FuncRefIndex, GlobalIndex, LabelIndex, LocalIndex, MemoryIndex, ModuleInternedTypeIndex,
    TableIndex, TagIndex, TypeIndex,
};

#[derive(Default)]
pub struct ModuleTranslation<'wasm> {
    /// The translated module.
    pub module: TranslatedModule,
    /// Information about the module's functions.
    pub function_bodies: PrimaryMap<DefinedFuncIndex, FunctionBodyData<'wasm>>,
    /// DWARF and other debug information parsed from the module.
    pub debug_info: DebugInfo<'wasm>,
    /// Required WASM features (proposals etc.) as self-reported by the module through the `wasm-features` custom section.
    /// Later on this could be used to determine which compiler/runtime features to enable, but
    /// for now we just use it to assert compatibility.
    pub required_features: WasmFeatures,
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
    pub tables: PrimaryMap<TableIndex, WasmTableType>,
    /// The memories declared in this module.
    pub memories: PrimaryMap<MemoryIndex, WasmMemoryType>,
    /// The globals declared in this module.
    pub globals: PrimaryMap<GlobalIndex, WasmGlobalType>,
    /// The tags declared in this module.
    pub tags: PrimaryMap<TagIndex, WasmTagType>,

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

#[derive(Debug)]
pub struct Function {
    /// The index of the function signature in the type section.
    pub signature: CanonicalizedTypeIndex,
    pub func_ref: FuncRefIndex,
}

pub struct FunctionBodyData<'wasm> {
    pub body: wasmparser::FunctionBody<'wasm>,
    pub validator: wasmparser::FuncToValidate<wasmparser::ValidatorResources>,
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
    pub ty: WasmEntityType,
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

// === impl Module ===

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

// === impl Function ===

impl Function {
    pub fn is_escaping(&self) -> bool {
        !self.func_ref.is_reserved_value()
    }
}

// === impl TableSegmentElements ===

impl TableSegmentElements {
    pub fn len(&self) -> usize {
        match self {
            TableSegmentElements::Functions(f) => f.len(),
            TableSegmentElements::Expressions(e) => e.len(),
        }
    }
}
