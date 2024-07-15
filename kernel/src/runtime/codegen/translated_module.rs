use crate::runtime::vmcontext::FuncRefIndex;
use alloc::boxed::Box;
use alloc::vec::Vec;
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::{EntityRef, PrimaryMap};
use cranelift_wasm::wasmparser::WasmFeatures;
use cranelift_wasm::{
    ConstExpr, DataIndex, DefinedFuncIndex, DefinedGlobalIndex, DefinedMemoryIndex,
    DefinedTableIndex, ElemIndex, EntityIndex, FuncIndex, Global, GlobalIndex, Memory, MemoryIndex,
    ModuleInternedTypeIndex, OwnedMemoryIndex, Table, TableIndex, TypeIndex,
};
use hashbrown::HashMap;

#[derive(Debug, Default)]
pub struct TranslatedModule<'wasm> {
    pub debug_info: DebugInfo<'wasm>,
    pub required_features: WasmFeatures,

    pub types: PrimaryMap<TypeIndex, ModuleInternedTypeIndex>,

    pub start: Option<FuncIndex>,

    pub imports: Vec<Import<'wasm>>,
    pub exports: HashMap<&'wasm str, EntityIndex>,

    pub functions: PrimaryMap<FuncIndex, FunctionType>,
    pub table_plans: PrimaryMap<TableIndex, TablePlan>,
    pub memory_plans: PrimaryMap<MemoryIndex, MemoryPlan>,
    pub globals: PrimaryMap<GlobalIndex, Global>,
    pub global_initializers: PrimaryMap<DefinedGlobalIndex, ConstExpr>,

    pub table_initializers: TableInitializers,
    pub passive_element_segments: PrimaryMap<ElemIndex, TableSegmentElements>,

    pub memory_initializers: MemoryInitializers<'wasm>,
    pub passive_data_segments: PrimaryMap<DataIndex, &'wasm [u8]>,

    pub num_imported_functions: u32,
    pub num_imported_tables: u32,
    pub num_imported_memories: u32,
    pub num_imported_globals: u32,

    pub num_escaped_funcs: u32,
}

#[derive(Debug, Default)]
pub struct DebugInfo<'wasm> {
    // pub names: Names<'wasm>,
    pub producers: Producers<'wasm>,
    pub dwarf: gimli::Dwarf<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_loc: gimli::DebugLoc<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_loclists: gimli::DebugLocLists<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_ranges: gimli::DebugRanges<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_rnglists: gimli::DebugRngLists<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_cu_index: gimli::DebugCuIndex<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_tu_index: gimli::DebugTuIndex<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
}

// #[derive(Debug, Default, Serialize, Deserialize)]
// pub struct Names<'wasm> {
//     pub module_name: Option<&'wasm str>,
//     pub func_names: HashMap<FuncIndex, &'wasm str>,
//     pub locals_names: HashMap<FuncIndex, HashMap<u32, &'wasm str>>,
//     pub global_names: HashMap<GlobalIndex, &'wasm str>,
//     pub data_names: HashMap<DataIndex, &'wasm str>,
// }

#[derive(Debug, Default)]
pub struct Producers<'wasm> {
    pub language: Vec<ProducersLanguageField<'wasm>>,
    pub processed_by: Vec<ProducersToolField<'wasm>>,
    pub sdk: Vec<ProducersSdkField<'wasm>>,
}

#[allow(unused)]
#[derive(Debug)]
pub struct ProducersLanguageField<'wasm> {
    pub name: ProducersLanguage<'wasm>,
    pub version: &'wasm str,
}

#[allow(unused)]
#[derive(Debug)]
pub enum ProducersLanguage<'wasm> {
    Wat,
    C,
    Cpp,
    Rust,
    JavaScript,
    Other(&'wasm str),
}

#[allow(unused)]
#[derive(Debug)]
pub struct ProducersToolField<'wasm> {
    pub name: ProducersTool<'wasm>,
    pub version: &'wasm str,
}

#[allow(unused)]
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

#[allow(unused)]
#[derive(Debug)]
pub struct ProducersSdkField<'wasm> {
    pub name: ProducersSdk<'wasm>,
    pub version: &'wasm str,
}

#[allow(unused)]
#[derive(Debug)]
pub enum ProducersSdk<'wasm> {
    Emscripten,
    Webpack,
    Other(&'wasm str),
}

#[derive(Debug, Copy, Clone)]
pub struct FunctionType {
    pub signature: ModuleInternedTypeIndex,
    pub func_ref: FuncRefIndex,
}

#[allow(unused)]
#[derive(Debug)]
pub struct Import<'wasm> {
    pub module: &'wasm str,
    pub name: &'wasm str,
    pub ty: EntityIndex,
}

#[derive(Debug)]
pub struct TablePlan {
    pub table: Table,
}

#[derive(Debug, Default)]
pub struct TableInitializers {
    pub initial_values: PrimaryMap<DefinedTableIndex, TableInitialValue>,
    pub segments: Vec<TableSegment>,
}

#[derive(Debug)]
pub enum TableInitialValue {
    RefNull,
    ConstExpr(ConstExpr),
}

#[derive(Debug)]
pub struct TableSegment {
    pub table_index: TableIndex,
    pub base: Option<GlobalIndex>,
    pub offset: u32,
    pub elements: TableSegmentElements,
}

#[derive(Debug)]
pub enum TableSegmentElements {
    Functions(Box<[FuncIndex]>),
    Expressions(Box<[ConstExpr]>),
}

#[derive(Debug)]
pub struct MemoryPlan {
    pub memory: Memory,
}

#[derive(Debug, Default)]
pub struct MemoryInitializers<'wasm> {
    pub runtime: Vec<MemoryInitializer<'wasm>>,
}

#[allow(unused)]
#[derive(Debug)]
pub struct MemoryInitializer<'wasm> {
    pub memory_index: MemoryIndex,
    pub base: Option<GlobalIndex>,
    pub offset: u32,
    pub bytes: &'wasm [u8],
}

impl<'wasm> TranslatedModule<'wasm> {
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
        u32::try_from(self.table_plans.len()).unwrap() - self.num_imported_tables
    }
    pub fn num_defined_memories(&self) -> u32 {
        u32::try_from(self.memory_plans.len()).unwrap() - self.num_imported_memories
    }
    pub fn num_owned_memories(&self) -> u32 {
        u32::try_from(
            self.memory_plans
                .iter()
                .skip(self.num_imported_memories as usize)
                .filter(|(_, mp)| !mp.memory.shared)
                .count(),
        )
        .unwrap()
    }
    pub fn num_defined_globals(&self) -> u32 {
        u32::try_from(self.globals.len()).unwrap() - self.num_imported_globals
    }
    pub fn num_escaped_funcs(&self) -> u32 {
        self.num_escaped_funcs
    }

    #[inline]
    pub fn function_index(&self, defined_func: DefinedFuncIndex) -> FuncIndex {
        FuncIndex::from_u32(self.num_imported_functions + defined_func.as_u32())
    }

    #[inline]
    pub fn defined_function_index(&self, func: FuncIndex) -> Option<DefinedFuncIndex> {
        if self.is_imported_function(func) {
            None
        } else {
            Some(DefinedFuncIndex::from_u32(
                func.as_u32() - self.num_imported_functions,
            ))
        }
    }

    #[inline]
    pub fn is_imported_function(&self, func: FuncIndex) -> bool {
        func.as_u32() < self.num_imported_functions
    }

    #[inline]
    pub fn table_index(&self, defined_table: DefinedTableIndex) -> TableIndex {
        TableIndex::from_u32(self.num_imported_tables + defined_table.as_u32())
    }

    #[inline]
    pub fn defined_table_index(&self, table: TableIndex) -> Option<DefinedTableIndex> {
        if self.is_imported_table(table) {
            None
        } else {
            Some(DefinedTableIndex::from_u32(
                table.as_u32() - self.num_imported_tables,
            ))
        }
    }

    #[inline]
    pub fn is_imported_table(&self, table: TableIndex) -> bool {
        table.as_u32() < self.num_imported_tables
    }

    #[inline]
    pub fn defined_tables(
        &self,
    ) -> impl ExactSizeIterator<Item = (DefinedTableIndex, &'_ TablePlan)> + '_ {
        self.table_plans
            .iter()
            .skip(self.num_imported_tables as usize)
            .map(|(index, table)| {
                let index = self.defined_table_index(index).unwrap();
                (index, table)
            })
    }

    #[inline]
    pub fn defined_memory_index(&self, mem: MemoryIndex) -> Option<DefinedMemoryIndex> {
        if self.is_imported_memory(mem) {
            None
        } else {
            Some(DefinedMemoryIndex::from_u32(
                mem.as_u32() - self.num_imported_memories,
            ))
        }
    }

    #[inline]
    pub fn owned_memory_index(&self, def_index: DefinedMemoryIndex) -> OwnedMemoryIndex {
        assert!(
            def_index.index() < self.memory_plans.len(),
            "non-shared memory must have an owned index"
        );

        // Once we know that the memory index is not greater than the number of
        // plans, we can iterate through the plans up to the memory index and
        // count how many are not shared (i.e., owned).
        let owned_memory_index = self
            .memory_plans
            .iter()
            .skip(self.num_imported_memories as usize)
            .take(def_index.index())
            .filter(|(_, mp)| !mp.memory.shared)
            .count();
        OwnedMemoryIndex::new(owned_memory_index)
    }

    #[inline]
    pub fn is_imported_memory(&self, mem: MemoryIndex) -> bool {
        mem.as_u32() < self.num_imported_memories
    }

    #[inline]
    pub fn defined_memories(
        &self,
    ) -> impl ExactSizeIterator<Item = (DefinedMemoryIndex, &'_ MemoryPlan)> + '_ {
        self.memory_plans
            .iter()
            .skip(self.num_imported_memories as usize)
            .map(|(index, memory)| {
                let index = self.defined_memory_index(index).unwrap();
                (index, memory)
            })
    }

    #[inline]
    pub fn defined_global_index(&self, global_index: GlobalIndex) -> Option<DefinedGlobalIndex> {
        if self.is_imported_global(global_index) {
            None
        } else {
            Some(DefinedGlobalIndex::from_u32(
                global_index.as_u32() - self.num_imported_globals,
            ))
        }
    }

    #[inline]
    pub fn is_imported_global(&self, global_index: GlobalIndex) -> bool {
        global_index.as_u32() < self.num_imported_globals
    }

    // /// Returns an iterator of all the imports in this module, along with their
    // /// module name, field name, and type that's being imported.
    // pub fn imports(&self) -> Imports<'wasm, '_> {
    //     Imports {
    //         module: self,
    //         index: 0,
    //     }
    // }
    //
    // /// Returns an iterator of all the imports in this module, along with their
    // /// module name, field name, and type that's being imported.
    // pub fn exports(&self) -> Exports<'wasm, '_> {
    //     Exports {
    //         module: self,
    //         exports: self.exports.iter(),
    //         index: 0,
    //     }
    // }
    // /// Returns the type of an item based on its index
    // pub fn type_of(&self, index: EntityIndex) -> EntityType {
    //     match index {
    //         EntityIndex::Global(i) => EntityType::Global(self.globals[i]),
    //         EntityIndex::Table(i) => EntityType::Table(self.table_plans[i].table),
    //         EntityIndex::Memory(i) => EntityType::Memory(self.memory_plans[i].memory),
    //         EntityIndex::Function(i) => {
    //             EntityType::Function(EngineOrModuleTypeIndex::Module(self.functions[i].signature))
    //         }
    //     }
    // }
}

impl FunctionType {
    pub fn is_escaping(self) -> bool {
        !self.func_ref.is_reserved_value()
    }
}

impl TablePlan {
    pub fn for_table(table: Table) -> Self {
        Self { table }
    }
}

impl MemoryPlan {
    pub fn for_memory(ty: cranelift_wasm::wasmparser::MemoryType) -> Self {
        let page_size_log2 = u8::try_from(ty.page_size_log2.unwrap_or(16)).unwrap();
        debug_assert!(
            page_size_log2 == 16 || page_size_log2 == 0,
            "invalid page_size_log2: {page_size_log2}; must be 16 or 0"
        );
        Self {
            memory: Memory {
                minimum: ty.initial,
                maximum: ty.maximum,
                shared: ty.shared,
                memory64: ty.memory64,
                page_size_log2,
            },
        }
    }
}

impl TableSegmentElements {
    pub fn len(&self) -> usize {
        match self {
            TableSegmentElements::Functions(inner) => inner.len(),
            TableSegmentElements::Expressions(inner) => inner.len(),
        }
    }
}
