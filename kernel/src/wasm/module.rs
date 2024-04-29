use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use cranelift_codegen::entity::{EntityRef, PrimaryMap};
use cranelift_wasm::{
    DefinedFuncIndex, DefinedGlobalIndex, DefinedMemoryIndex, EntityIndex, FuncIndex, Global,
    GlobalIndex, Memory, MemoryIndex, ModuleInternedTypeIndex, OwnedMemoryIndex, Table, TableIndex,
    TypeIndex,
};

#[derive(Debug)]
pub struct Import<'wasm> {
    /// Name of this import
    pub module: &'wasm str,
    /// The field name projection of this import
    pub field: &'wasm str,
    /// Where this import will be placed, which also has type information
    /// about the import.
    pub index: EntityIndex,
}

#[derive(Debug)]
pub struct FunctionType {
    /// The type of this function, indexed into the module-wide type tables for
    /// a module compilation.
    pub signature: ModuleInternedTypeIndex,
    // /// The index into the funcref table, if present. Note that this is
    // /// `reserved_value()` if the function does not escape from a module.
    // pub func_ref: FuncRefIndex,
}

#[derive(Debug)]
pub struct TablePlan {
    /// The WebAssembly table description
    pub table: Table,
}

#[derive(Debug)]
pub struct MemoryPlan {
    /// The WebAssembly linear memory description.
    pub memory: Memory,
}

#[derive(Debug, Default)]
pub struct Module<'wasm> {
    /// The name of the module if any
    pub name: Option<&'wasm str>,
    /// The start function of the module if any
    pub start: Option<FuncIndex>,

    /// Imports declared in the wasm module
    pub imports: Vec<Import<'wasm>>,
    /// Exports declared in the wasm module
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

    pub num_imported_funcs: u32,
    pub num_imported_tables: u32,
    pub num_imported_memories: u32,
    pub num_imported_globals: u32,
    pub num_escaped_funcs: u32,
}

impl<'wasm> Module<'wasm> {
    #[inline]
    pub fn func_index(&self, defined_func: DefinedFuncIndex) -> FuncIndex {
        FuncIndex::from_u32(self.num_imported_funcs + defined_func.as_u32())
    }

    #[inline]
    pub fn defined_func_index(&self, func: FuncIndex) -> Option<DefinedFuncIndex> {
        if func.as_u32() < self.num_imported_funcs {
            None
        } else {
            Some(DefinedFuncIndex::from_u32(
                func.as_u32() - self.num_imported_funcs,
            ))
        }
    }

    #[inline]
    pub fn is_imported_function(&self, index: FuncIndex) -> bool {
        index.as_u32() < self.num_imported_funcs
    }

    #[inline]
    pub fn defined_memory_index(&self, memory_index: MemoryIndex) -> Option<DefinedMemoryIndex> {
        if self.is_defined_memory(memory_index) {
            None
        } else {
            Some(DefinedMemoryIndex::from_u32(
                memory_index.as_u32() - self.num_imported_memories,
            ))
        }
    }

    #[inline]
    pub fn is_defined_memory(&self, memory_index: MemoryIndex) -> bool {
        memory_index.as_u32() < self.num_imported_memories
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
    pub fn defined_global_index(&self, global_index: GlobalIndex) -> Option<DefinedGlobalIndex> {
        if self.is_defined_global(global_index) {
            None
        } else {
            Some(DefinedGlobalIndex::from_u32(
                global_index.as_u32() - self.num_imported_globals,
            ))
        }
    }

    #[inline]
    pub fn is_defined_global(&self, global_index: GlobalIndex) -> bool {
        global_index.as_u32() < self.num_imported_globals
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
