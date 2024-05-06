mod func_env;
mod module_env;

use crate::runtime::FuncRefIndex;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use cranelift_codegen::entity::{EntityRef, PrimaryMap};
use cranelift_codegen::packed_option::ReservedValue;
use cranelift_wasm::wasmparser::MemoryType;
use cranelift_wasm::{
    ConstExpr, DefinedFuncIndex, DefinedGlobalIndex, DefinedMemoryIndex, EngineOrModuleTypeIndex,
    EntityIndex, FuncIndex, Global, GlobalIndex, Memory, MemoryIndex, ModuleInternedTypeIndex,
    OwnedMemoryIndex, Table, TableIndex, TypeIndex,
};
pub use func_env::FuncEnvironment;
pub use module_env::{FunctionBodyInput, ModuleEnvironment, ModuleTranslation};

#[derive(Debug, Default)]
pub struct TranslatedModule<'wasm> {
    pub name: Option<&'wasm str>,

    /// The start function of the module if any
    pub start_func: Option<FuncIndex>,
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

    pub global_initializers: PrimaryMap<DefinedGlobalIndex, ConstExpr>,

    pub num_imported_funcs: u32,
    pub num_imported_tables: u32,
    pub num_imported_memories: u32,
    pub num_imported_globals: u32,
    pub num_escaped_funcs: u32,
}

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

/// A table plan describes how we plan to allocate, instantiate and handle a given table
#[derive(Debug)]
pub struct TablePlan {
    /// The WebAssembly table description
    pub table: Table,
}

#[derive(Debug)]
pub struct FunctionType {
    /// The type of this function, indexed into the module-wide type tables for
    /// a module compilation.
    pub signature: ModuleInternedTypeIndex,
    /// The index into the funcref table, if present. Note that this is
    /// `reserved_value()` if the function does not escape from a module.
    pub func_ref: FuncRefIndex,
}

/// A type of an item in a wasm module where an item is typically something that
/// can be exported.
#[allow(missing_docs)]
#[derive(Clone, Debug)]
pub enum EntityType {
    /// A global variable with the specified content type
    Global(Global),
    /// A linear memory with the specified limits
    Memory(Memory),
    /// A table with the specified element type and limits
    Table(Table),
    /// A function type where the index points to the type section and records a
    /// function signature.
    Function(EngineOrModuleTypeIndex),
}

impl<'wasm> TranslatedModule<'wasm> {
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

    /// Returns an iterator of all the imports in this module, along with their
    /// module name, field name, and type that's being imported.
    pub fn imports(&self) -> Imports<'wasm, '_> {
        Imports {
            module: self,
            index: 0,
        }
    }

    /// Returns an iterator of all the imports in this module, along with their
    /// module name, field name, and type that's being imported.
    pub fn exports(&self) -> Exports<'wasm, '_> {
        Exports {
            module: self,
            exports: self.exports.iter(),
            index: 0,
        }
    }

    /// Returns the type of an item based on its index
    pub fn type_of(&self, index: EntityIndex) -> EntityType {
        match index {
            EntityIndex::Global(i) => EntityType::Global(self.globals[i]),
            EntityIndex::Table(i) => EntityType::Table(self.table_plans[i].table),
            EntityIndex::Memory(i) => EntityType::Memory(self.memory_plans[i].memory),
            EntityIndex::Function(i) => {
                EntityType::Function(EngineOrModuleTypeIndex::Module(self.functions[i].signature))
            }
        }
    }

    pub fn num_escaped_funcs(&self) -> u32 {
        self.num_escaped_funcs
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

    pub fn try_static_init(&self, _alignment: usize, _max_always_allowed: usize) -> Vec<u8> {
        todo!()
    }
}

impl TablePlan {
    pub fn for_table(table: Table) -> Self {
        Self { table }
    }
}

/// A memory plan describes how we plan to allocate, instantiate and handle a given memory
#[derive(Debug)]
pub struct MemoryPlan {
    /// The WebAssembly linear memory description.
    pub memory: Memory,
}

impl MemoryPlan {
    pub fn for_memory_type(ty: MemoryType) -> Self {
        Self {
            memory: Memory {
                minimum: ty.initial,
                maximum: ty.maximum,
                shared: ty.shared,
                memory64: ty.memory64,
            },
        }
    }
}

impl FunctionType {
    /// Returns whether this function's type is one that "escapes" the current
    /// module, meaning that the function is exported, used in `ref.func`, used
    /// in a table, etc.
    pub fn is_escaping(&self) -> bool {
        !self.func_ref.is_reserved_value()
    }
}

pub struct Imports<'wasm, 'module> {
    module: &'module TranslatedModule<'wasm>,
    index: usize,
}

impl<'wasm, 'module> Iterator for Imports<'wasm, 'module> {
    type Item = (&'wasm str, &'wasm str, EntityType);

    fn next(&mut self) -> Option<Self::Item> {
        let i = self.module.imports.get(self.index)?;
        self.index += 1;

        Some((i.module, i.field, self.module.type_of(i.index)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.module.imports.len(), Some(self.module.imports.len()))
    }
}

impl<'module, 'wasm> ExactSizeIterator for Imports<'module, 'wasm> {}

pub struct Exports<'wasm, 'module> {
    module: &'module TranslatedModule<'wasm>,
    exports: alloc::collections::btree_map::Iter<'module, &'wasm str, EntityIndex>,
    index: usize,
}

impl<'wasm, 'module> Iterator for Exports<'wasm, 'module> {
    type Item = (&'wasm str, EntityType);

    fn next(&mut self) -> Option<Self::Item> {
        let (name, index) = self.exports.next()?;
        self.index += 1;

        Some((name, self.module.type_of(*index)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.module.imports.len(), Some(self.module.imports.len()))
    }
}

impl<'module, 'wasm> ExactSizeIterator for Exports<'module, 'wasm> {}
