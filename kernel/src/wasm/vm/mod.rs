// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod builtins;
mod code_object;
mod const_eval;
mod instance;
mod instance_alloc;
mod memory;
mod provenance;
mod table;
mod vmcontext;
mod vmshape;
mod mmap_vec;

use alloc::vec::Vec;
use core::ptr::NonNull;

use crate::wasm::indices::DefinedMemoryIndex;
use crate::wasm::translate::TranslatedModule;
pub use code_object::CodeObject;
pub use const_eval::ConstExprEvaluator;
pub use instance::{Instance, InstanceAndStore, InstanceHandle};
pub use instance_alloc::{InstanceAllocator, PlaceholderAllocatorDontUse};
pub use memory::Memory;
pub use table::{Table, TableElement};
pub use vmcontext::*;
pub use vmshape::{StaticVMShape, VMShape};
pub use mmap_vec::MmapVec;
pub use provenance::VmPtr;
use crate::wasm::translate;

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
    ///
    /// Note that exported functions cannot be a null funcref, so this is a
    /// non-null pointer.
    pub func_ref: NonNull<VMFuncRef>,
}
// As part of the contract for using `ExportFunction`, synchronization
// properties must be upheld. Therefore, despite containing raw pointers,
// it is declared as Send/Sync.
unsafe impl Send for ExportedFunction {}
unsafe impl Sync for ExportedFunction {}

#[derive(Debug, Clone)]
pub struct ExportedTable {
    /// The address of the table descriptor.
    pub definition: NonNull<VMTableDefinition>,
    /// Pointer to the containing `VMContext`.
    pub vmctx: NonNull<VMContext>,
    pub table: translate::Table,
}
// See docs on send/sync for `ExportFunction` above.
unsafe impl Send for ExportedTable {}
unsafe impl Sync for ExportedTable {}

/// A memory export value.
#[derive(Debug, Clone)]
pub struct ExportedMemory {
    /// The address of the memory descriptor.
    pub definition: NonNull<VMMemoryDefinition>,
    /// Pointer to the containing `VMContext`.
    pub vmctx: NonNull<VMContext>,
    /// The index at which the memory is defined within the `vmctx`.
    pub index: DefinedMemoryIndex,
    pub memory: translate::Memory,
}
// See docs on send/sync for `ExportFunction` above.
unsafe impl Send for ExportedMemory {}
unsafe impl Sync for ExportedMemory {}

/// A global export value.
#[derive(Debug, Clone)]
pub struct ExportedGlobal {
    /// The address of the global storage.
    pub definition: NonNull<VMGlobalDefinition>,
    /// Pointer to the containing `VMContext`. May be null for host-created
    /// globals.
    pub vmctx: Option<NonNull<VMContext>>,
    pub global: translate::Global,
}
// See docs on send/sync for `ExportFunction` above.
unsafe impl Send for ExportedGlobal {}
unsafe impl Sync for ExportedGlobal {}

/// A tag export value.
#[derive(Debug, Clone)]
pub struct ExportedTag {
    /// The address of the global storage.
    pub definition: NonNull<VMTagDefinition>,
    pub tag: translate::Tag
}
// See docs on send/sync for `ExportFunction` above.
unsafe impl Send for ExportedTag {}
unsafe impl Sync for ExportedTag {}

#[derive(Debug, Default)]
pub struct Imports {
    pub functions: Vec<VMFunctionImport>,
    pub tables: Vec<VMTableImport>,
    pub memories: Vec<VMMemoryImport>,
    pub globals: Vec<VMGlobalImport>,
    pub tags: Vec<VMTagImport>,
}

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
