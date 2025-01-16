//! ```text
//! struct VMContext {
//!     magic: u32,
//!     _padding: u32, // on 64-bit platforms
//!     builtin_functions: *const VMBuiltinFunctionsArray,
//!     type_ids: *const VMSharedTypeIndex,
//!     stack_limit: *const u8,
//!     last_wasm_exit_fp: *const u8,
//!     last_wasm_exit_pc: *const u8,
//!     last_wasm_entry_fp: *const u8,
//!     func_refs: [VMFuncRef; num_escaped_funcs],
//!     imported_functions: [VMFunctionImport; num_imported_functions)],
//!     imported_tables: [VMTableImport; num_imported_tables],
//!     imported_memories: [VMMemoryImport; num_imported_memories],
//!     imported_globals: [VMGlobalImport; num_imported_globals],
//!     tables: [VMTableDefinition; num_defined_tables],
//!     memories: [VMMemoryDefinition; num_defined_memories],
//!     globals: [VMGlobalDefinition; num_defined_globals],
//! }
//! ```

use crate::u32_offset_of;
use crate::wasm::indices::{
    DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, FuncIndex, FuncRefIndex,
    GlobalIndex, MemoryIndex, TableIndex,
};
use crate::wasm::runtime::vmcontext::{
    VMFuncRef, VMFunctionImport, VMGlobalDefinition, VMGlobalImport, VMMemoryDefinition,
    VMMemoryImport, VMTableDefinition, VMTableImport,
};
use crate::wasm::translate::TranslatedModule;
use core::fmt;
use core::mem::offset_of;
use cranelift_entity::packed_option::ReservedValue;

/// Offsets to fields in the `VMContext` structure that are statically known (i.e. do not
/// depend on the size of the module)
#[derive(Clone)]
pub struct StaticVMOffsets {
    ptr_size: u8,
}

impl fmt::Debug for StaticVMOffsets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticVMOffsets")
            .field("vmctx_magic", &self.size())
            .field("vmctx_builtin_functions", &self.vmctx_builtin_functions())
            .field("vmctx_stack_limit", &self.vmctx_stack_limit())
            .field("vmctx_last_wasm_exit_fp", &self.vmctx_last_wasm_exit_fp())
            .field("vmctx_last_wasm_exit_pc", &self.vmctx_last_wasm_exit_pc())
            .field("vmctx_last_wasm_entry_fp", &self.vmctx_last_wasm_entry_fp())
            .finish()
    }
}

impl StaticVMOffsets {
    pub fn new(ptr_size: u8) -> Self {
        Self { ptr_size }
    }

    /// Offset of the `magic` value in a `VMContext`.
    #[inline]
    #[expect(clippy::unused_self, reason = "accessor")]
    pub const fn vmctx_magic(&self) -> u8 {
        // This is required by the implementation of `VMContext::instance` and
        // `VMContext::instance_mut`. If this value changes then those locations
        // need to be updated.
        0
    }

    /// Offset of the `builtin_functions` field in a `VMContext`.
    #[inline]
    pub const fn vmctx_builtin_functions(&self) -> u8 {
        self.vmctx_magic() + self.ptr_size
    }

    /// Offset of the `type_ids` field in a `VMContext`.
    #[inline]
    pub const fn vmctx_type_ids(&self) -> u8 {
        self.vmctx_builtin_functions() + self.ptr_size
    }

    /// Offset of the `stack_limit` field in a `VMContext`.
    #[inline]
    pub const fn vmctx_stack_limit(&self) -> u8 {
        self.vmctx_type_ids() + self.ptr_size
    }

    /// Offset of the `last_wasm_exit_fp` field in a `VMContext`.
    #[inline]
    pub const fn vmctx_last_wasm_exit_fp(&self) -> u8 {
        self.vmctx_stack_limit() + self.ptr_size
    }

    /// Offset of the `last_wasm_exit_pc` field in a `VMContext`.
    #[inline]
    pub const fn vmctx_last_wasm_exit_pc(&self) -> u8 {
        self.vmctx_last_wasm_exit_fp() + self.ptr_size
    }

    /// Offset of the `last_wasm_entry_fp` field in a `VMContext`.
    #[inline]
    pub const fn vmctx_last_wasm_entry_fp(&self) -> u8 {
        self.vmctx_last_wasm_exit_pc() + self.ptr_size
    }

    /// The size of the statically known part of a `VMContext`.
    #[inline]
    const fn size(&self) -> u8 {
        self.vmctx_last_wasm_entry_fp() + self.ptr_size
    }

    /// Return the size of `VMSharedTypeIndex`.
    #[inline]
    #[expect(clippy::unused_self, reason = "accessor")]
    pub const fn size_of_vmshared_type_index(&self) -> u8 {
        4
    }
}

/// Offsets to fields in the `VMContext` structure.
#[derive(Debug)]
pub struct VMOffsets {
    num_imported_funcs: u32,
    num_imported_tables: u32,
    num_imported_memories: u32,
    num_imported_globals: u32,
    num_defined_tables: u32,
    num_defined_memories: u32,
    num_defined_globals: u32,
    num_escaped_funcs: u32,

    // offsets
    pub static_: StaticVMOffsets,
    func_refs: u32,
    imported_functions: u32,
    imported_tables: u32,
    imported_memories: u32,
    imported_globals: u32,
    tables: u32,
    memories: u32,
    globals: u32,

    size: u32,
}

impl VMOffsets {
    /// Calculate the `VMOffsets` for a given `module`.
    pub fn for_module(ptr_size: u8, module: &TranslatedModule) -> Self {
        let static_ = StaticVMOffsets::new(ptr_size);

        let mut offset = u32::from(static_.size());
        let mut member_offset = |size_of_member: u32| -> u32 {
            let out = offset;
            offset += size_of_member;
            out
        };

        Self {
            num_imported_funcs: module.num_imported_functions(),
            num_imported_tables: module.num_imported_tables(),
            num_imported_memories: module.num_imported_memories(),
            num_imported_globals: module.num_imported_globals(),
            num_defined_tables: module.num_defined_tables(),
            num_defined_memories: module.num_defined_memories(),
            num_defined_globals: module.num_defined_globals(),
            num_escaped_funcs: module.num_escaped_funcs(),

            // offsets
            static_,
            func_refs: member_offset(u32_size_of::<VMFuncRef>() * module.num_escaped_funcs()),
            imported_functions: member_offset(
                u32_size_of::<VMFunctionImport>() * module.num_imported_functions(),
            ),
            imported_tables: member_offset(
                u32_size_of::<VMTableImport>() * module.num_imported_tables(),
            ),
            imported_memories: member_offset(
                u32_size_of::<VMMemoryImport>() * module.num_imported_memories(),
            ),
            imported_globals: member_offset(
                u32_size_of::<VMGlobalImport>() * module.num_imported_globals(),
            ),
            tables: member_offset(u32_size_of::<VMTableDefinition>() * module.num_defined_tables()),
            memories: member_offset(
                u32_size_of::<VMMemoryDefinition>() * module.num_defined_memories(),
            ),
            globals: member_offset(
                u32_size_of::<VMGlobalDefinition>() * module.num_defined_globals(),
            ),

            size: offset,
        }
    }

    /// The offset of the `func_refs` array in `VMContext`.
    #[inline]
    pub fn vmctx_func_refs_begin(&self) -> u32 {
        self.func_refs
    }

    /// The offset of the `imported_functions` array in `VMContext`.
    #[inline]
    pub fn vmctx_imported_functions_begin(&self) -> u32 {
        self.imported_functions
    }

    /// The offset of the `imported_tables` array in `VMContext`.
    #[inline]
    pub fn vmctx_imported_tables_begin(&self) -> u32 {
        self.imported_tables
    }

    /// The offset of the `imported_memories` array in `VMContext`.
    #[inline]
    pub fn vmctx_imported_memories_begin(&self) -> u32 {
        self.imported_memories
    }

    /// The offset of the `imported_globals` array in `VMContext`.
    #[inline]
    pub fn vmctx_imported_globals_begin(&self) -> u32 {
        self.imported_globals
    }

    /// The offset of the `tables` array in `VMContext`.
    #[inline]
    pub fn vmctx_tables_begin(&self) -> u32 {
        self.tables
    }

    /// The offset of the `memories` array in `VMContext`.
    #[inline]
    pub fn vmctx_memories_begin(&self) -> u32 {
        self.memories
    }

    /// The offset of the `globals` array in `VMContext`.
    #[inline]
    pub fn vmctx_globals_begin(&self) -> u32 {
        self.globals
    }

    /// Offset of the `index`nth `VMFuncRef` in the `func_refs` array.
    #[inline]
    pub fn vmctx_vmfunc_ref(&self, index: FuncRefIndex) -> u32 {
        assert!(!index.is_reserved_value()); // Non-escaping functions are marked using the reserved value.
        assert!(index.as_u32() < self.num_escaped_funcs);
        self.vmctx_func_refs_begin() + index.as_u32() * u32_size_of::<VMFuncRef>()
    }
    /// Offset of the `index`nth `VMFuncRef`s `array_call` field.
    #[inline]
    pub fn vmctx_vmfunc_ref_array_call(&self, index: FuncRefIndex) -> u32 {
        self.vmctx_vmfunc_ref(index) + u32_offset_of!(VMFuncRef, array_call)
    }
    /// Offset of the `index`nth `VMFuncRef`s `wasm_call` field.
    #[inline]
    pub fn vmctx_vmfunc_ref_wasm_call(&self, index: FuncRefIndex) -> u32 {
        self.vmctx_vmfunc_ref(index) + u32_offset_of!(VMFuncRef, wasm_call)
    }
    /// Offset of the `index`nth `VMFuncRef`s `vmctx` field.
    #[inline]
    pub fn vmctx_vmfunc_ref_vmctx(&self, index: FuncRefIndex) -> u32 {
        self.vmctx_vmfunc_ref(index) + u32_offset_of!(VMFuncRef, vmctx)
    }
    /// Offset of the `index`nth `VMFuncRef`s `type_index` field.
    #[inline]
    pub fn vmctx_vmfunc_ref_type_index(&self, index: FuncRefIndex) -> u32 {
        self.vmctx_vmfunc_ref(index) + u32_offset_of!(VMFuncRef, type_index)
    }

    /// Offset of the `index`nth `VMFunctionImport` in the `imported_functions` array.
    #[inline]
    pub fn vmctx_vmfunction_import(&self, index: FuncIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_funcs);
        self.vmctx_imported_functions_begin() + index.as_u32() * u32_size_of::<VMFunctionImport>()
    }
    /// Offset of the `index`nth `VMFunctionImport`s `wasm_call` field.
    #[inline]
    pub fn vmctx_vmfunction_import_wasm_call(&self, index: FuncIndex) -> u32 {
        self.vmctx_vmfunction_import(index) + u32_offset_of!(VMFunctionImport, wasm_call)
    }
    /// Offset of the `index`nth `VMFunctionImport`s `array_call` field.
    #[inline]
    pub fn vmctx_vmfunction_import_array_call(&self, index: FuncIndex) -> u32 {
        self.vmctx_vmfunction_import(index) + u32_offset_of!(VMFunctionImport, array_call)
    }
    /// Offset of the `index`nth `VMFunctionImport`s `vmctx` field.
    #[inline]
    pub fn vmctx_vmfunction_import_vmctx(&self, index: FuncIndex) -> u32 {
        self.vmctx_vmfunction_import(index) + u32_offset_of!(VMFunctionImport, vmctx)
    }

    /// Offset of the `index`nth `VMTableImport` in the `imported_tables` array.
    #[inline]
    pub fn vmctx_vmtable_import(&self, index: TableIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_tables);
        self.vmctx_imported_tables_begin() + index.as_u32() * u32_size_of::<VMTableImport>()
    }
    /// Offset of the `index`nth `VMTableImport`s `from` field.
    #[inline]
    pub fn vmctx_vmtable_import_from(&self, index: TableIndex) -> u32 {
        self.vmctx_vmtable_import(index) + u32_offset_of!(VMTableImport, from)
    }
    /// Offset of the `index`nth `VMTableImport`s `vmctx` field.
    #[inline]
    pub fn vmctx_vmtable_import_vmctx(&self, index: TableIndex) -> u32 {
        self.vmctx_vmtable_import(index) + u32_offset_of!(VMTableImport, vmctx)
    }

    /// Offset of the `index`nth `VMMemoryImport` in the `imported_memories` array.
    #[inline]
    pub fn vmctx_vmmemory_import(&self, index: MemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_memories);
        self.vmctx_imported_memories_begin() + index.as_u32() * u32_size_of::<VMMemoryImport>()
    }
    /// Offset of the `index`nth `VMMemoryImport`s `from` field.
    #[inline]
    pub fn vmctx_vmmemory_import_from(&self, index: MemoryIndex) -> u32 {
        self.vmctx_vmmemory_import(index) + u32_offset_of!(VMMemoryImport, from)
    }
    /// Offset of the `index`nth `VMMemoryImport`s `vmctx` field.
    #[inline]
    pub fn vmctx_vmmemory_import_vmctx(&self, index: MemoryIndex) -> u32 {
        self.vmctx_vmmemory_import(index) + u32_offset_of!(VMMemoryImport, vmctx)
    }

    /// Offset of the `index`nth `VMGlobalImport` in the `imported_globals` array.
    #[inline]
    pub fn vmctx_vmglobal_import(&self, index: GlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_globals);
        self.vmctx_imported_globals_begin() + index.as_u32() * u32_size_of::<VMGlobalImport>()
    }
    /// Offset of the `index`nth `VMGlobalImport`s `from` field.
    #[inline]
    pub fn vmctx_vmglobal_import_from(&self, index: GlobalIndex) -> u32 {
        self.vmctx_vmglobal_import(index) + u32_offset_of!(VMGlobalImport, from)
    }
    /// Offset of the `index`nth `VMGlobalImport`s `vmctx` field.
    #[inline]
    pub fn vmctx_vmglobal_import_vmctx(&self, index: GlobalIndex) -> u32 {
        self.vmctx_vmglobal_import(index) + u32_offset_of!(VMGlobalImport, vmctx)
    }

    /// Offset of the `index`nth `VMTableDefinition` in the `tables` array.
    #[inline]
    pub fn vmctx_vmtable_definition(&self, index: DefinedTableIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_tables);
        self.vmctx_tables_begin() + index.as_u32() * u32_size_of::<VMTableDefinition>()
    }
    /// Offset of the `index`nth `VMTableDefinition`s `base` field.
    #[inline]
    pub fn vmctx_vmtable_definition_base(&self, index: DefinedTableIndex) -> u32 {
        self.vmctx_vmtable_definition(index) + u32_offset_of!(VMTableDefinition, base)
    }
    /// Offset of the `index`nth `VMTableDefinition`s `current_length` field.
    #[inline]
    pub fn vmctx_vmtable_definition_current_length(&self, index: DefinedTableIndex) -> u32 {
        self.vmctx_vmtable_definition(index) + u32_offset_of!(VMTableDefinition, current_length)
    }

    /// Offset of the `index`nth `VMMemoryDefinition` in the `memories` array.
    #[inline]
    pub fn vmctx_vmmemory_definition(&self, index: DefinedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_memories);
        self.vmctx_memories_begin() + index.as_u32() * u32_size_of::<VMMemoryDefinition>()
    }
    /// Offset of the `index`nth `VMMemoryDefinition`s `base` field.
    #[inline]
    pub fn vmctx_vmmemory_definition_base(&self, index: DefinedMemoryIndex) -> u32 {
        self.vmctx_vmmemory_definition(index) + u32_offset_of!(VMMemoryDefinition, base)
    }
    /// Offset of the `index`nth `VMMemoryDefinition`s `current_length` field.
    #[inline]
    pub fn vmctx_vmmemory_definition_current_length(&self, index: DefinedMemoryIndex) -> u32 {
        self.vmctx_vmmemory_definition(index) + u32_offset_of!(VMMemoryDefinition, current_length)
    }

    /// Offset of the `index`nth `VMGlobalDefinition` in the `globals` array.
    #[inline]
    pub fn vmctx_vmglobal_definition(&self, index: DefinedGlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_globals);
        self.vmctx_globals_begin() + index.as_u32() * u32_size_of::<VMGlobalDefinition>()
    }
    #[inline]
    pub fn size(&self) -> u32 {
        self.size
    }
}

/// Like `mem::size_of` but returns `u32` instead of `usize`
/// # Panics
///
/// Panics if the size of `T` is greater than `u32::MAX`.
fn u32_size_of<T: Sized>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap()
}
