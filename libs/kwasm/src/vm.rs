// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod builtins;
mod code_object;
mod const_eval;
mod instance;
mod instance_allocator;
mod memory;
mod mmap;
mod provenance;
mod table;
mod vmcontext;
mod vmcontext_shape;

use alloc::vec::Vec;
use core::ptr::NonNull;

pub use code_object::CodeObject;
pub use const_eval::{ConstEvalContext, ConstExprEvaluator};
pub use instance::{Instance, InstanceAndStore, InstanceHandle};
pub use instance_allocator::InstanceAllocator;
pub use memory::Memory;
pub use mmap::{Mmap, Permissions, RawMmap, RawMmapVTable};
pub use provenance::{VmPtr, VmSafe};
pub use table::{Table, TableElement, TableElementType};
pub use vmcontext::{
    VM_ARRAY_CALL_HOST_FUNC_MAGIC, VMArrayCallHostFuncContext, VMArrayCallNative, VMCONTEXT_MAGIC,
    VMContext, VMFuncRef, VMFunctionImport, VMGlobalDefinition, VMGlobalImport, VMMemoryDefinition,
    VMMemoryImport, VMOpaqueContext, VMStoreContext, VMTableDefinition, VMTableImport,
    VMTagDefinition, VMTagImport, VMVal, VMWasmCallFunction,
};
pub use vmcontext_shape::{StaticVMContextShape, VMContextShape};

use crate::wasm::{TranslatedModule, WasmGlobalType, WasmMemoryType, WasmTableType, WasmTagType};

/// The value of an export passed from one instance to another.
#[derive(Debug, Clone)]
pub enum Export {
    /// A function export value.
    Function(ExportedFunction),
    /// A table export value.
    Table(ExportedTable),
    /// A memory export value.
    Memory(ExportedMemory),
    /// A global export value.
    Global(ExportedGlobal),
    /// A tag export value.
    Tag(ExportedTag),
}

/// A function export value.
#[derive(Debug, Clone)]
pub struct ExportedFunction {
    /// Note that exported functions cannot be a null funcref, so this is a
    /// non-null pointer.
    pub func_ref: NonNull<VMFuncRef>,
}
// Safety: As part of the contract for using `ExportFunction`, synchronization
// properties must be upheld. Therefore, despite containing raw pointers,
// it is declared as Send/Sync.
unsafe impl Send for ExportedFunction {}
// Safety: see above
unsafe impl Sync for ExportedFunction {}

/// A table export value.
#[derive(Debug, Clone)]
pub struct ExportedTable {
    /// The address of the table descriptor.
    pub definition: NonNull<VMTableDefinition>,
    /// Pointer to the containing `VMContext`.
    pub vmctx: NonNull<VMContext>,
    pub table: WasmTableType,
}
// Safety: see docs on send/sync for `ExportedFunction` above.
unsafe impl Send for ExportedTable {}
// Safety: see docs on send/sync for `ExportedFunction` above.
unsafe impl Sync for ExportedTable {}

/// A memory export value.
#[derive(Debug, Clone)]
pub struct ExportedMemory {
    /// The address of the memory descriptor.
    pub definition: NonNull<VMMemoryDefinition>,
    /// Pointer to the containing `VMContext`.
    pub vmctx: NonNull<VMContext>,
    pub memory: WasmMemoryType,
}
// Safety: see docs on send/sync for `ExportedFunction` above.
unsafe impl Send for ExportedMemory {}
// Safety: see docs on send/sync for `ExportedFunction` above.
unsafe impl Sync for ExportedMemory {}

/// A global export value.
#[derive(Debug, Clone)]
pub struct ExportedGlobal {
    /// The address of the global storage.
    pub definition: NonNull<VMGlobalDefinition>,
    /// Pointer to the containing `VMContext`. May be null for host-created
    /// globals.
    pub vmctx: Option<NonNull<VMContext>>,
    pub global: WasmGlobalType,
}
// Safety: see docs on send/sync for `ExportedFunction` above.
unsafe impl Send for ExportedGlobal {}
// Safety: see docs on send/sync for `ExportedFunction` above.
unsafe impl Sync for ExportedGlobal {}

/// A tag export value.
#[derive(Debug, Clone)]
pub struct ExportedTag {
    /// The address of the global storage.
    pub definition: NonNull<VMTagDefinition>,
    pub tag: WasmTagType,
}
// Safety: see docs on send/sync for `ExportedFunction` above.
unsafe impl Send for ExportedTag {}
// Safety: see docs on send/sync for `ExportedFunction` above.
unsafe impl Sync for ExportedTag {}

#[derive(Debug, Default)]
pub struct Imports {
    pub functions: Vec<VMFunctionImport>,
    pub tables: Vec<VMTableImport>,
    pub memories: Vec<VMMemoryImport>,
    pub globals: Vec<VMGlobalImport>,
    pub tags: Vec<VMTagImport>,
}

// === impl Imports ===

impl Imports {
    pub(crate) fn with_capacity_for(raw: &TranslatedModule) -> Self {
        let mut this = Self::default();

        this.functions.reserve(raw.num_imported_functions as usize);
        this.tables.reserve(raw.num_imported_tables as usize);
        this.memories.reserve(raw.num_imported_memories as usize);
        this.globals.reserve(raw.num_imported_globals as usize);
        this.tags.reserve(raw.num_imported_tags as usize);

        this
    }
}
