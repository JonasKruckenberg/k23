use crate::wasm::{MEMORY_GUARD_SIZE, WASM32_MAX_PAGES, WASM64_MAX_PAGES};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ops::Range;
use cranelift_codegen::entity::{entity_impl, EntityRef, PrimaryMap};
use cranelift_codegen::packed_option::ReservedValue;
use cranelift_wasm::{
    ConstExpr, DataIndex, DefinedFuncIndex, DefinedGlobalIndex, DefinedMemoryIndex,
    DefinedTableIndex, ElemIndex, EntityIndex, FuncIndex, Global, GlobalIndex, Memory, MemoryIndex,
    ModuleInternedTypeIndex, OwnedMemoryIndex, Table, TableIndex, TypeIndex,
};

#[derive(Default, Debug)]
pub struct Module<'wasm> {
    pub name: Option<&'wasm str>,

    pub start_func: Option<FuncIndex>,

    pub imports: Vec<Import<'wasm>>,
    pub exports: BTreeMap<&'wasm str, EntityIndex>,

    /// Types declared in the wasm module.
    pub types: PrimaryMap<TypeIndex, ModuleInternedTypeIndex>,

    /// Types of functions, imported and local.
    pub functions: PrimaryMap<FuncIndex, FunctionType>,
    /// WebAssembly tables.
    pub table_plans: PrimaryMap<TableIndex, TablePlan>,
    /// WebAssembly linear memory plans.
    pub memory_plans: PrimaryMap<MemoryIndex, MemoryPlan>,
    /// WebAssembly global variables.
    pub globals: PrimaryMap<GlobalIndex, Global>,

    // /// WebAssembly table element initializers for locally-defined tables.
    // pub table_initializers: PrimaryMap<DefinedTableIndex, ConstExpr>,
    /// WebAssembly global initializers for locally-defined globals.
    pub global_initializers: PrimaryMap<DefinedGlobalIndex, ConstExpr>,
    /// WebAssembly passive elements.
    pub passive_elements: Vec<TableSegmentElements>,
    /// The map from passive element index (element segment index space) to index in `passive_elements`.
    pub passive_elements_map: BTreeMap<ElemIndex, usize>,
    /// The map from passive data index (data segment index space) to offset range.
    pub passive_data_map: BTreeMap<DataIndex, Range<u32>>,

    pub num_imported_funcs: u32,
    pub num_imported_tables: u32,
    pub num_imported_memories: u32,
    pub num_imported_globals: u32,
    pub num_escaped_funcs: u32,
}

/// Index into the funcref table within a VMContext for a function.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct FuncRefIndex(u32);
entity_impl!(FuncRefIndex);

#[derive(Debug)]
pub struct FunctionType {
    /// The type of this function, indexed into the module-wide type tables for
    /// a module compilation.
    pub signature: ModuleInternedTypeIndex,
    /// The index into the funcref table, if present. Note that this is
    /// `reserved_value()` if the function does not escape from a module.
    pub func_ref: FuncRefIndex,
}

impl FunctionType {
    /// Returns whether this function's type is one that "escapes" the current
    /// module, meaning that the function is exported, used in `ref.func`, used
    /// in a table, etc.
    pub fn is_escaping(&self) -> bool {
        !self.func_ref.is_reserved_value()
    }
}

#[derive(Debug)]
pub struct Import<'wasm> {
    /// Name of this import
    pub name: &'wasm str,
    /// The field name projection of this import
    pub field: &'wasm str,
    /// Where this import will be placed, which also has type information
    /// about the import.
    pub index: EntityIndex,
}

/// Elements of a table segment, either a list of functions or list of arbitrary
/// expressions.
#[derive(Clone, Debug)]
pub enum TableSegmentElements {
    /// A sequential list of functions where `FuncIndex::reserved_value()`
    /// indicates a null function.
    Functions(Box<[FuncIndex]>),
    /// Arbitrary expressions, aka either functions, null or a load of a global.
    Expressions(Box<[ConstExpr]>),
}

impl TableSegmentElements {
    /// Returns the number of elements in this segment.
    pub fn len(&self) -> u32 {
        match self {
            Self::Functions(s) => s.len() as u32,
            Self::Expressions(s) => s.len() as u32,
        }
    }
}

/// Implementation styles for WebAssembly linear memory.
#[derive(Debug)]
pub enum MemoryStyle {
    /// The address space for this linear memory is reserved upfront.
    Static { max_pages: u64 },
    /// The memory can be remapped and resized
    Dynamic,
}

#[derive(Debug)]
pub struct MemoryPlan {
    /// The WebAssembly linear memory description.
    pub memory: Memory,
    /// Our chosen implementation style.
    pub style: MemoryStyle,
    /// Chosen size of a guard page before the linear memory allocation.
    pub pre_guard_size: u64,
}

#[derive(Debug)]
pub struct TablePlan {
    /// The WebAssembly table description
    pub table: Table,
}

impl<'wasm> Module<'wasm> {
    pub fn func_index(&self, defined_func: DefinedFuncIndex) -> FuncIndex {
        FuncIndex::from_u32(self.num_imported_funcs + defined_func.as_u32())
    }

    pub fn defined_func_index(&self, func: FuncIndex) -> Option<DefinedFuncIndex> {
        if func.as_u32() < self.num_imported_funcs {
            None
        } else {
            Some(DefinedFuncIndex::from_u32(
                func.as_u32() - self.num_imported_funcs,
            ))
        }
    }

    pub fn is_imported_function(&self, index: FuncIndex) -> bool {
        index.as_u32() < self.num_imported_funcs
    }

    pub fn defined_table_index(&self, global: TableIndex) -> Option<DefinedTableIndex> {
        if global.as_u32() < self.num_imported_tables {
            None
        } else {
            Some(DefinedTableIndex::from_u32(
                global.as_u32() - self.num_imported_tables,
            ))
        }
    }

    pub fn is_imported_table(&self, index: TableIndex) -> bool {
        index.as_u32() < self.num_imported_tables
    }

    pub fn defined_memory_index(&self, memory: MemoryIndex) -> Option<DefinedMemoryIndex> {
        if memory.as_u32() < self.num_imported_memories {
            None
        } else {
            Some(DefinedMemoryIndex::from_u32(
                memory.as_u32() - self.num_imported_memories,
            ))
        }
    }

    pub fn owned_memory_index(&self, memory: DefinedMemoryIndex) -> OwnedMemoryIndex {
        assert!(
            memory.index() < self.memory_plans.len(),
            "non-shared memory must have an owned index"
        );

        // Once we know that the memory index is not greater than the number of
        // plans, we can iterate through the plans up to the memory index and
        // count how many are not shared (i.e., owned).
        let owned_memory_index = self
            .memory_plans
            .iter()
            .skip(self.num_imported_memories as usize)
            .take(memory.index())
            .filter(|(_, mp)| !mp.memory.shared)
            .count();
        OwnedMemoryIndex::new(owned_memory_index)
    }

    pub fn is_imported_memory(&self, index: MemoryIndex) -> bool {
        index.as_u32() < self.num_imported_memories
    }

    pub fn defined_global_index(&self, global: GlobalIndex) -> Option<DefinedGlobalIndex> {
        if global.as_u32() < self.num_imported_globals {
            None
        } else {
            Some(DefinedGlobalIndex::from_u32(
                global.as_u32() - self.num_imported_globals,
            ))
        }
    }

    pub fn is_imported_global(&self, index: GlobalIndex) -> bool {
        index.as_u32() < self.num_imported_globals
    }

    pub fn num_imported_funcs(&self) -> u32 {
        self.num_imported_funcs
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
        self.table_plans.len() as u32
    }
    pub fn num_defined_memories(&self) -> u32 {
        self.memory_plans.len() as u32
    }
    pub fn num_owned_memories(&self) -> u32 {
        self.memory_plans.len() as u32 - self.num_imported_memories
    }
    pub fn num_defined_globals(&self) -> u32 {
        self.globals.len() as u32
    }
}

impl MemoryPlan {
    /// Draw up a plan for implementing a `Memory`.
    pub fn for_memory(memory: Memory) -> Self {
        let absolute_max_pages = if memory.memory64 {
            WASM64_MAX_PAGES
        } else {
            WASM32_MAX_PAGES
        };
        let max_pages = core::cmp::min(
            memory.maximum.unwrap_or(absolute_max_pages),
            absolute_max_pages,
        );

        assert!(memory.minimum <= max_pages);

        Self {
            memory,
            style: MemoryStyle::Static { max_pages },
            pre_guard_size: MEMORY_GUARD_SIZE,
        }
    }
}

impl TablePlan {
    pub fn for_table(table: Table) -> Self {
        Self { table }
    }
}
