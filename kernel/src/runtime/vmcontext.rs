//! # VMContext
//!
//! There are many bits of data that the JIT code needs at run-time, such as pointers to tables,
//! constants, pointers to imported objects and runtime configuration.
//!
//! All this JIT state is kept inside a `VMContext` struct that currently looks like this:
//! ```rust
//! #[repr(C)]
//! struct VMContext {
//!     magic: usize,
//!     builtins: *mut VMBuiltinFunctionsArray,
//!     tables: [VMTableDefinition; module.num_defined_tables],
//!     memories: [*mut VMMemoryDefinition; module.num_defined_memories],
//!     owned_memories: [VMMemoryDefinition; module.num_owned_memories],
//!     globals: [VMGlobalDefinition; module.num_defined_globals],
//!     func_refs: [VMFuncRef; module.num_escaped_funcs],
//!     imported_functions: [VMFunctionImport; module.num_imported_functions],
//!     imported_tables: [VMTableImport; module.num_imported_tables],
//!     imported_memories: [VMMemoryImport; module.num_imported_memories],
//!     imported_globals: [VMGlobalImport; module.num_imported_globals],
//!     stack_limit: usize,
//!     last_wasm_exit_fp: usize,
//!     last_wasm_exit_pc: usize,
//!     last_wasm_entry_sp: usize,
//! }
//! ```
//!
//! As you can see, the final size of a `VMContext` depends on number of defined items in the given
//! WASM module. This means we can't actually define `VMContext` as a Rust struct.
//! During compilation populate a `VMContextPlan` struct that describes how the corresponding
//! `VMContext` will be laid out in memory, this `VMContextPlan` instance is then used for
//! pointer + offset calculations to access the structs fields.
//!
//     /// The value of the frame pointer register when we last called from Wasm to
//     /// the host.
//     ///
//     /// Maintained by our Wasm-to-host trampoline, and cleared just before
//     /// calling into Wasm in `catch_traps`.
//     ///
//     /// This member is `0` when Wasm is actively running and has not called out
//     /// to the host.
//     ///
//     /// Used to find the start of a a contiguous sequence of Wasm frames when
//     /// walking the stack.
//     pub last_wasm_exit_fp: UnsafeCell<usize>,
//
//     /// The last Wasm program counter before we called from Wasm to the host.
//     ///
//     /// Maintained by our Wasm-to-host trampoline, and cleared just before
//     /// calling into Wasm in `catch_traps`.
//     ///
//     /// This member is `0` when Wasm is actively running and has not called out
//     /// to the host.
//     ///
//     /// Used when walking a contiguous sequence of Wasm frames.
//     pub last_wasm_exit_pc: UnsafeCell<usize>,
//
//     /// The last host stack pointer before we called into Wasm from the host.
//     ///
//     /// Maintained by our host-to-Wasm trampoline, and cleared just before
//     /// calling into Wasm in `catch_traps`.
//     ///
//     /// This member is `0` when Wasm is actively running and has not called out
//     /// to the host.
//     ///
//     /// When a host function is wrapped into a `wasmtime::Func`, and is then
//     /// called from the host, then this member has the sentinal value of `-1 as
//     /// usize`, meaning that this contiguous sequence of Wasm frames is the
//     /// empty sequence, and it is not safe to dereference the
//     /// `last_wasm_exit_fp`.
//     ///
//     /// Used to find the end of a contiguous sequence of Wasm frames when
//     /// walking the stack.
//     pub last_wasm_entry_sp: UnsafeCell<usize>,

use crate::runtime::wasm2ir::Module;
use core::mem;
use core::mem::offset_of;
use core::sync::atomic::AtomicUsize;
use cranelift_codegen::isa::TargetIsa;
use cranelift_wasm::{
    DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, FuncIndex, GlobalIndex, MemoryIndex,
    OwnedMemoryIndex, TableIndex,
};

pub const VMCONTEXT_MAGIC: u32 = u32::from_le_bytes(*b"vmcx");

/// A VMContext plan describes how we plan to allocate, instantiate and handle the VMContext for a
/// given module.
///
/// This struct is used by compilation code (namely the `FuncEnvironment`) to access the offsets from the
/// global `vmctx` pointer.
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

#[repr(C)]
struct VMTableDefinition {}

#[repr(C)]
struct VMMemoryDefinition {
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
struct VMGlobalDefinition {}

#[repr(C)]
struct VMFuncRef {}

#[repr(C)]
struct VMFunctionImport {}

#[repr(C)]
struct VMTableImport {}

#[repr(C)]
struct VMMemoryImport {}

#[repr(C)]
struct VMGlobalImport {}

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
        self.stack_limit
    }
    #[inline]
    pub fn last_wasm_exit_pc(&self) -> u32 {
        self.stack_limit
    }
    #[inline]
    pub fn last_wasm_entry_sp(&self) -> u32 {
        self.stack_limit
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
