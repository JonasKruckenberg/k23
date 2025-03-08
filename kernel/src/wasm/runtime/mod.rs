mod builtins;
mod code_memory;
pub mod code_registry;
mod const_eval;
mod instance;
mod instance_allocator;
mod memory;
mod mmap_vec;
mod owned_vmcontext;
mod table;
mod vmcontext;
mod vmoffsets;

use alloc::vec::Vec;
use core::ptr::NonNull;

use crate::wasm::runtime::vmcontext::VMGlobalDefinition;
use crate::wasm::translate::{GlobalDesc, MemoryDesc, TableDesc, TranslatedModule};
pub use code_memory::CodeMemory;
pub use const_eval::ConstExprEvaluator;
pub use instance::Instance;
pub use instance_allocator::InstanceAllocator;
pub use memory::Memory;
pub use mmap_vec::MmapVec;
pub use owned_vmcontext::OwnedVMContext;
pub use table::Table;
pub use vmcontext::{
    VMCONTEXT_MAGIC, VM_HOST_CONTEXT_MAGIC, VMContext, VMFuncRef, VMFunctionImport, VMGlobalImport, VMMemoryDefinition,
    VMMemoryImport, VMOpaqueContext, VMTableDefinition, VMTableImport, VMVal, VMWasmCallFunction
};
pub use vmoffsets::{StaticVMOffsets, VMOffsets};

pub enum Export {
    Function(ExportedFunction),
    Table(ExportedTable),
    Memory(ExportedMemory),
    Global(ExportedGlobal),
}

/// A function export value.
#[derive(Debug, Clone, Copy)]
pub struct ExportedFunction {
    /// The `VMFuncRef` for this exported function.
    ///
    /// Note that exported functions cannot be a null funcref, so this is a
    /// non-null pointer.
    pub func_ref: NonNull<VMFuncRef>,
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Send for ExportedFunction {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Sync for ExportedFunction {}

/// A table export value.
#[derive(Debug, Clone)]
pub struct ExportedTable {
    /// The address of the table descriptor.
    pub definition: *mut VMTableDefinition,
    /// Pointer to the containing `VMContext`.
    pub vmctx: *mut VMContext,
    /// The table declaration, used for compatibility checking.
    pub table: TableDesc,
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Send for ExportedTable {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Sync for ExportedTable {}

/// A memory export value.
#[derive(Debug, Clone)]
pub struct ExportedMemory {
    /// The address of the memory descriptor.
    pub definition: *mut VMMemoryDefinition,
    /// Pointer to the containing `VMContext`.
    pub vmctx: *mut VMContext,
    /// The memory declaration, used for compatibility checking.
    pub memory: MemoryDesc,
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Send for ExportedMemory {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Sync for ExportedMemory {}

/// A global export value.
#[derive(Debug, Clone)]
pub struct ExportedGlobal {
    /// The address of the global storage.
    pub definition: *mut VMGlobalDefinition,
    /// Pointer to the containing `VMContext`. May be null for host-created
    /// globals.
    pub vmctx: *mut VMContext,
    /// The global declaration, used for compatibility checking.
    pub ty: GlobalDesc,
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Send for ExportedGlobal {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Sync for ExportedGlobal {}

#[derive(Debug, Default)]
pub struct Imports {
    pub functions: Vec<VMFunctionImport>,
    pub tables: Vec<VMTableImport>,
    pub memories: Vec<VMMemoryImport>,
    pub globals: Vec<VMGlobalImport>,
}

impl Imports {
    pub(crate) fn with_capacity_for(raw: &TranslatedModule) -> Self {
        let mut this = Self::default();

        this.functions.reserve(raw.num_imported_functions as usize);
        this.tables.reserve(raw.num_imported_tables as usize);
        this.memories.reserve(raw.num_imported_memories as usize);
        this.globals.reserve(raw.num_imported_globals as usize);

        this
    }
}
