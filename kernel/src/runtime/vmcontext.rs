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
//!     scratch: VMScratchSpace
//! }

use super::translate::TranslatedModule;
use core::mem;
use core::mem::offset_of;
use core::sync::atomic::AtomicUsize;
use cranelift_codegen::entity::entity_impl;
use cranelift_codegen::isa::TargetIsa;
use cranelift_wasm::{
    DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, FuncIndex, GlobalIndex, MemoryIndex,
    OwnedMemoryIndex, TableIndex, WasmValType,
};
use vmm::VirtualAddress;

pub const VMCONTEXT_MAGIC: u32 = u32::from_le_bytes(*b"vmcx");

/// Index into the funcref table within a VMContext for a function.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct FuncRefIndex(u32);
entity_impl!(FuncRefIndex);

#[repr(C)]
pub struct VMContext {
    ptr: *const u8,
}

pub union ValRaw {
    pub i32: i32,
    pub i64: i64,
    pub f32: u32,
    pub f64: u64,
    pub v128: [u8; 16],
    // pub funcref: *mut c_void,
    // pub externref: u32,
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
pub struct VMGlobalDefinition {
    pub data: [u8; 16],
}

impl VMGlobalDefinition {
    pub unsafe fn from_val_raw(val_raw: ValRaw) -> Self {
        Self { data: val_raw.v128 }
    }

    pub unsafe fn to_val_raw(&self, wasm_ty: &WasmValType) -> ValRaw {
        match wasm_ty {
            WasmValType::I32 => ValRaw {
                i32: *self.as_i32(),
            },
            WasmValType::I64 => ValRaw {
                i64: *self.as_i64(),
            },
            WasmValType::F32 => ValRaw {
                f32: *self.as_f32_bits(),
            },
            WasmValType::F64 => ValRaw {
                f64: *self.as_f64_bits(),
            },
            WasmValType::V128 => ValRaw {
                v128: self.data.clone(),
            },
            WasmValType::Ref(_) => todo!(),
        }
    }

    /// Return a reference to the value as an i32.
    pub unsafe fn as_i32(&self) -> &i32 {
        &*(self.data.as_ref().as_ptr().cast::<i32>())
    }

    /// Return a mutable reference to the value as an i32.
    pub unsafe fn as_i32_mut(&mut self) -> &mut i32 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<i32>())
    }

    /// Return a reference to the value as a u32.
    pub unsafe fn as_u32(&self) -> &u32 {
        &*(self.data.as_ref().as_ptr().cast::<u32>())
    }

    /// Return a mutable reference to the value as an u32.
    pub unsafe fn as_u32_mut(&mut self) -> &mut u32 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u32>())
    }

    /// Return a reference to the value as an i64.
    pub unsafe fn as_i64(&self) -> &i64 {
        &*(self.data.as_ref().as_ptr().cast::<i64>())
    }

    /// Return a mutable reference to the value as an i64.
    pub unsafe fn as_i64_mut(&mut self) -> &mut i64 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<i64>())
    }

    /// Return a reference to the value as an u64.
    pub unsafe fn as_u64(&self) -> &u64 {
        &*(self.data.as_ref().as_ptr().cast::<u64>())
    }

    /// Return a mutable reference to the value as an u64.
    pub unsafe fn as_u64_mut(&mut self) -> &mut u64 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u64>())
    }

    /// Return a reference to the value as an f32.
    pub unsafe fn as_f32(&self) -> &f32 {
        &*(self.data.as_ref().as_ptr().cast::<f32>())
    }

    /// Return a mutable reference to the value as an f32.
    pub unsafe fn as_f32_mut(&mut self) -> &mut f32 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<f32>())
    }

    /// Return a reference to the value as f32 bits.
    pub unsafe fn as_f32_bits(&self) -> &u32 {
        &*(self.data.as_ref().as_ptr().cast::<u32>())
    }

    /// Return a mutable reference to the value as f32 bits.
    pub unsafe fn as_f32_bits_mut(&mut self) -> &mut u32 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u32>())
    }

    /// Return a reference to the value as an f64.
    pub unsafe fn as_f64(&self) -> &f64 {
        &*(self.data.as_ref().as_ptr().cast::<f64>())
    }

    /// Return a mutable reference to the value as an f64.
    pub unsafe fn as_f64_mut(&mut self) -> &mut f64 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<f64>())
    }

    /// Return a reference to the value as f64 bits.
    pub unsafe fn as_f64_bits(&self) -> &u64 {
        &*(self.data.as_ref().as_ptr().cast::<u64>())
    }

    /// Return a mutable reference to the value as f64 bits.
    pub unsafe fn as_f64_bits_mut(&mut self) -> &mut u64 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u64>())
    }

    /// Return a reference to the value as an u128.
    pub unsafe fn as_u128(&self) -> &u128 {
        &*(self.data.as_ref().as_ptr().cast::<u128>())
    }

    /// Return a mutable reference to the value as an u128.
    pub unsafe fn as_u128_mut(&mut self) -> &mut u128 {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<u128>())
    }

    /// Return a reference to the value as u128 bits.
    pub unsafe fn as_u128_bits(&self) -> &[u8; 16] {
        &*(self.data.as_ref().as_ptr().cast::<[u8; 16]>())
    }

    /// Return a mutable reference to the value as u128 bits.
    pub unsafe fn as_u128_bits_mut(&mut self) -> &mut [u8; 16] {
        &mut *(self.data.as_mut().as_mut_ptr().cast::<[u8; 16]>())
    }
}

#[repr(C)]
pub struct VMFuncRef {
    pub native_call: VirtualAddress,
}

#[repr(C)]
pub struct VMFunctionImport {}

#[repr(C)]
pub struct VMTableImport {}

#[repr(C)]
pub struct VMMemoryImport {}

#[repr(C)]
pub struct VMGlobalImport {}

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
    pub fn for_module(isa: &dyn TargetIsa, module: &TranslatedModule) -> Self {
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
            tables: member_offset(size_of_u32::<VMTableDefinition>() * module.num_defined_tables()),
            memories: member_offset(ptr_size * module.num_defined_memories()),
            owned_memories: member_offset(
                size_of_u32::<VMMemoryDefinition>() * module.num_owned_memories(),
            ),
            globals: member_offset(
                size_of_u32::<VMGlobalDefinition>() * module.num_defined_globals(),
            ),
            func_refs: member_offset(size_of_u32::<VMFuncRef>() * module.num_escaped_funcs()),
            imported_functions: member_offset(
                size_of_u32::<VMFunctionImport>() * module.num_imported_funcs(),
            ),
            imported_tables: member_offset(
                size_of_u32::<VMTableImport>() * module.num_imported_tables(),
            ),
            imported_memories: member_offset(
                size_of_u32::<VMMemoryImport>() * module.num_imported_memories(),
            ),
            imported_globals: member_offset(
                size_of_u32::<VMGlobalImport>() * module.num_imported_globals(),
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
    pub fn vmctx_magic(&self) -> u32 {
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
