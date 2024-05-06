use crate::rt::translate::{MemoryPlan, TablePlan};
use crate::rt::{VMFuncRef, VMGlobalDefinition, VMMemoryDefinition, VMTableDefinition};
use core::ptr::NonNull;
use cranelift_wasm::{DefinedMemoryIndex, Global};

pub enum Export {
    Function(ExportFunction),
    Table(ExportTable),
    Memory(ExportMemory),
    Global(ExportGlobal),
}

pub struct ExportFunction {
    pub func_ref: NonNull<VMFuncRef>,
}

pub struct ExportTable {
    pub definition: NonNull<VMTableDefinition>,
    pub table: TablePlan,
}

pub struct ExportMemory {
    pub definition: NonNull<VMMemoryDefinition>,
    pub memory: MemoryPlan,
    pub index: DefinedMemoryIndex,
}

pub struct ExportGlobal {
    pub definition: NonNull<VMGlobalDefinition>,
    pub global: Global,
}
