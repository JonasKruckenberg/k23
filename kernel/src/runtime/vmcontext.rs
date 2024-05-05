use crate::runtime::translate::Module;
use core::mem;
use core::mem::offset_of;
use core::sync::atomic::AtomicUsize;
use cranelift_codegen::isa::TargetIsa;
use cranelift_wasm::{
    DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, FuncIndex, GlobalIndex, MemoryIndex,
    OwnedMemoryIndex, TableIndex,
};

pub const VMCONTEXT_MAGIC: u32 = u32::from_le_bytes(*b"vmcx");

#[repr(C)]
pub struct VMContext {
    ptr: *const u8,
}

#[repr(C)]
pub struct VMTableDefinition {}

#[repr(C)]
pub struct VMMemoryDefinition {
    /// The start address.
    pub base: *mut u8,
    /// The current logical size of this linear memory in bytes.
    ///
    /// This is atomic because shared memories must be able to grow their length
    /// atomically. For relaxed access, see
    /// [`VMMemoryDefinition::current_length()`].
    pub current_length: AtomicUsize,
    /// The address space identifier of the memory
    pub asid: usize,
}

#[repr(C)]
pub struct VMGlobalDefinition {}

#[repr(C)]
pub struct VMFuncRef {}

#[repr(C)]
pub struct VMFunctionImport {}

#[repr(C)]
pub struct VMTableImport {}

#[repr(C)]
pub struct VMMemoryImport {}

#[repr(C)]
pub struct VMGlobalImport {}

/// VMContextOffsets describes how the VMContext for the corresponding module will be laid out at runtime.
///
/// This struct is used by compilation code (namely the `FuncEnvironment`) to access the offsets from the
/// global `vmctx` pointer.
#[derive(Debug)]
pub struct VMContextOffsets {
    num_imported_funcs: u32,
    num_imported_tables: u32,
    num_imported_memories: u32,
    num_imported_globals: u32,
    num_defined_tables: u32,
    num_defined_memories: u32,
    num_owned_memories: u32,
    num_defined_globals: u32,
    num_escaped_funcs: u32,
    /// target ISA pointer size in bytes
    ptr_size: u32,
    size: u32,

    // offsets
    magic: u32,
    builtins: u32,
    tables: u32,
    memories: u32,
    owned_memories: u32,
    globals: u32,
    func_refs: u32,
    imported_functions: u32,
    imported_tables: u32,
    imported_memories: u32,
    imported_globals: u32,
    stack_limit: u32,
    last_wasm_exit_fp: u32,
    last_wasm_exit_pc: u32,
    last_wasm_entry_sp: u32,
}

impl VMContextOffsets {
    pub fn for_module(isa: &dyn TargetIsa, module: &Module) -> Self {
        let mut offset = 0;

        let mut member_offset = |size_of_member: u32| -> u32 {
            let out = offset;
            offset += size_of_member;
            out
        };

        let ptr_size = isa.pointer_bytes() as u32;

        Self {
            num_imported_funcs: module.num_imported_funcs(),
            num_imported_tables: module.num_imported_tables(),
            num_imported_memories: module.num_imported_memories(),
            num_imported_globals: module.num_imported_globals(),
            num_defined_tables: module.num_defined_tables(),
            num_defined_memories: module.num_defined_memories(),
            num_owned_memories: module.num_owned_memories(),
            num_defined_globals: module.num_defined_globals(),
            num_escaped_funcs: module.num_escaped_funcs(),
            ptr_size,

            // offsets
            magic: member_offset(ptr_size),
            builtins: member_offset(ptr_size),
            tables: member_offset(module.num_defined_tables() * size_of_u32::<VMTableDefinition>()),
            memories: member_offset(module.num_defined_memories() * ptr_size),
            owned_memories: member_offset(
                module.num_owned_memories() * size_of_u32::<VMMemoryDefinition>(),
            ),
            globals: member_offset(
                module.num_defined_globals() * size_of_u32::<VMGlobalDefinition>(),
            ),
            func_refs: member_offset(module.num_escaped_funcs() * size_of_u32::<VMFuncRef>()),
            imported_functions: member_offset(
                module.num_imported_funcs() * size_of_u32::<VMFunctionImport>(),
            ),
            imported_tables: member_offset(
                module.num_imported_tables() * size_of_u32::<VMTableImport>(),
            ),
            imported_memories: member_offset(
                module.num_imported_memories() * size_of_u32::<VMMemoryImport>(),
            ),
            imported_globals: member_offset(
                module.num_imported_globals() * size_of_u32::<VMGlobalImport>(),
            ),
            stack_limit: member_offset(ptr_size),
            last_wasm_exit_fp: member_offset(ptr_size),
            last_wasm_exit_pc: member_offset(ptr_size),
            last_wasm_entry_sp: member_offset(ptr_size),
            size: offset,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    #[inline]
    pub fn magic(&self) -> u32 {
        self.magic
    }
    #[inline]
    pub fn builtins(&self) -> u32 {
        self.builtins
    }
    #[inline]
    pub fn vmtable_definition(&self, index: DefinedTableIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_tables);
        self.tables + index.as_u32() * size_of_u32::<VMTableDefinition>()
    }
    #[inline]
    pub fn vmmemory_pointer(&self, index: DefinedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_memories);
        self.memories + index.as_u32() * self.ptr_size
    }
    #[inline]
    pub fn vmmemory_definition(&self, index: OwnedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_owned_memories);
        self.owned_memories + index.as_u32() * size_of_u32::<VMMemoryDefinition>()
    }
    #[inline]
    pub fn vmglobal_definition(&self, index: DefinedGlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_globals);
        self.globals + index.as_u32() * size_of_u32::<VMGlobalDefinition>()
    }
    #[inline]
    pub fn vmfunc_ref(&self, index: FuncIndex) -> u32 {
        assert!(index.as_u32() < self.num_escaped_funcs);
        self.func_refs + index.as_u32() * size_of_u32::<VMFuncRef>()
    }
    #[inline]
    pub fn vmfunction_import(&self, index: FuncIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_funcs);
        self.imported_functions + index.as_u32() * size_of_u32::<VMFunctionImport>()
    }
    #[inline]
    pub fn vmtable_import(&self, index: TableIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_tables);
        self.imported_tables + index.as_u32() * size_of_u32::<VMTableImport>()
    }
    #[inline]
    pub fn vmmemory_import(&self, index: MemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_memories);
        self.imported_memories + index.as_u32() * size_of_u32::<VMMemoryImport>()
    }
    #[inline]
    pub fn vmglobal_import(&self, index: GlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_globals);
        self.imported_globals + index.as_u32() * size_of_u32::<VMGlobalImport>()
    }
    #[inline]
    pub fn stack_limit(&self) -> u32 {
        self.stack_limit
    }
    #[inline]
    pub fn last_wasm_exit_fp(&self) -> u32 {
        self.last_wasm_exit_fp
    }
    #[inline]
    pub fn last_wasm_exit_pc(&self) -> u32 {
        self.last_wasm_exit_pc
    }
    #[inline]
    pub fn last_wasm_entry_sp(&self) -> u32 {
        self.last_wasm_entry_sp
    }

    /// Return the offset to the `base` field in `VMMemoryDefinition` index `index`.
    #[inline]
    pub fn vmmemory_definition_base(&self, index: OwnedMemoryIndex) -> u32 {
        self.vmmemory_definition(index) + offset_of!(VMMemoryDefinition, base) as u32
    }
    /// Return the offset to the `current_length` field in `VMMemoryDefinition` index `index`.
    #[inline]
    pub fn vmmemory_definition_current_length(&self, index: OwnedMemoryIndex) -> u32 {
        self.vmmemory_definition(index) + offset_of!(VMMemoryDefinition, current_length) as u32
    }
}

fn size_of_u32<T: Sized>() -> u32 {
    mem::size_of::<T>() as u32
}
