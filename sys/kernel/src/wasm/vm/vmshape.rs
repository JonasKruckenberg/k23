// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Currently the `VMContext` allocation by field looks like this:
//!
//! ```
//! struct VMContext {
//!      // Fixed-width data comes first so the calculation of the offset of
//!      // these fields is a compile-time constant when using `HostPtr`.
//!      magic: u32,
//!      _padding: u32, //! (On 64-bit systems)
//!      vm_store_context: *const VMStoreContext,
//!      builtin_functions: VmPtr<VMBuiltinFunctionsArray>,
//!      callee: VmPtr<VMFunctionBody>,
//!      epoch_ptr: *mut AtomicU64,
//!      gc_heap_base: *mut u8,
//!      gc_heap_bound: *mut u8,
//!      gc_heap_data: *mut T, //! Collector-specific pointer
//!      type_ids: *const VMSharedTypeIndex,
//!
//!      // Variable-width fields come after the fixed-width fields above. Place
//!      // memory-related items first as they're some of the most frequently
//!      // accessed items and minimizing their offset in this structure can
//!      // shrink the size of load/store instruction offset immediates on
//!      // platforms like x64 (e.g. fit in an 8-bit offset instead
//!      // of needing a 32-bit offset)
//!      imported_memories: [VMMemoryImport; module.num_imported_memories],
//!      memories: [VmPtr<VMMemoryDefinition>; module.num_defined_memories],
//!      owned_memories: [VMMemoryDefinition; module.num_owned_memories],
//!      imported_functions: [VMFunctionImport; module.num_imported_functions],
//!      imported_tables: [VMTable; module.num_imported_tables],
//!      imported_globals: [VMGlobalImport; module.num_imported_globals],
//!      imported_tags: [VMTagImport; module.num_imported_tags],
//!      tables: [VMTableDefinition; module.num_defined_tables],
//!      globals: [VMGlobalDefinition; module.num_defined_globals],
//!      tags: [VMTagDefinition; module.num_defined_tags],
//!      func_refs: [VMFuncRef; module.num_escaped_funcs],
//! }
//! ```

use crate::wasm::indices::{
    DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, DefinedTagIndex, FuncIndex,
    FuncRefIndex, GlobalIndex, MemoryIndex, OwnedMemoryIndex, TableIndex, TagIndex,
};
use crate::wasm::translate::TranslatedModule;
use crate::wasm::utils::u8_size_of;
use crate::wasm::vm::provenance::VmPtr;
use crate::wasm::vm::vmcontext::{
    VMFuncRef, VMFunctionImport, VMGlobalDefinition, VMGlobalImport, VMMemoryDefinition,
    VMMemoryImport, VMTableDefinition, VMTableImport, VMTagDefinition, VMTagImport,
};

pub struct StaticVMShape;

#[derive(Debug, Clone)]
pub struct VMShape {
    /// The number of imported functions in the module.
    pub num_imported_functions: u32,
    /// The number of imported tables in the module.
    pub num_imported_tables: u32,
    /// The number of imported memories in the module.
    pub num_imported_memories: u32,
    /// The number of imported globals in the module.
    pub num_imported_globals: u32,
    /// The number of imported tags in the module.
    pub num_imported_tags: u32,
    /// The number of defined tables in the module.
    pub num_defined_tables: u32,
    /// The number of defined memories in the module.
    pub num_defined_memories: u32,
    /// The number of memories owned by the module instance.
    pub num_owned_memories: u32,
    /// The number of defined globals in the module.
    pub num_defined_globals: u32,
    /// The number of defined tags in the module.
    pub num_defined_tags: u32,
    /// The number of escaped functions in the module, the size of the func_refs
    /// array.
    pub num_escaped_funcs: u32,

    // precalculated offsets of various member fields
    imported_functions: u32,
    imported_tables: u32,
    imported_memories: u32,
    imported_globals: u32,
    imported_tags: u32,
    defined_tables: u32,
    defined_memories: u32,
    owned_memories: u32,
    defined_globals: u32,
    defined_tags: u32,
    defined_func_refs: u32,
    size: u32,
}

impl StaticVMShape {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "pointers larger than 255 bytes dont exist"
    )]
    const fn ptr_size(&self) -> u8 {
        size_of::<usize>() as u8
    }

    /// Return the offset to the `magic` value in this `VMContext`.
    #[inline]
    pub const fn vmctx_magic(&self) -> u8 {
        // This is required by the implementation of `VMContext::instance` and
        // `VMContext::instance_mut`. If this value changes then those locations
        // need to be updated.
        0
    }

    /// Return the offset to the `VMStoreContext` structure
    #[inline]
    pub const fn vmctx_store_context(&self) -> u8 {
        self.vmctx_magic() + self.ptr_size()
    }

    /// Return the offset to the `VMBuiltinFunctionsArray` structure
    #[inline]
    pub const fn vmctx_builtin_functions(&self) -> u8 {
        self.vmctx_store_context() + self.ptr_size()
    }

    /// Return the offset to the `callee` member in this `VMContext`.
    #[inline]
    pub const fn vmctx_callee(&self) -> u8 {
        self.vmctx_builtin_functions() + self.ptr_size()
    }

    /// Return the offset to the `*const AtomicU64` epoch-counter
    /// pointer.
    #[inline]
    pub const fn vmctx_epoch_ptr(&self) -> u8 {
        self.vmctx_callee() + self.ptr_size()
    }

    /// Return the offset to the GC heap base in this `VMContext`.
    #[inline]
    pub const fn vmctx_gc_heap_base(&self) -> u8 {
        self.vmctx_epoch_ptr() + self.ptr_size()
    }

    /// Return the offset to the GC heap bound in this `VMContext`.
    #[inline]
    pub const fn vmctx_gc_heap_bound(&self) -> u8 {
        self.vmctx_gc_heap_base() + self.ptr_size()
    }

    /// Return the offset to the `*mut T` collector-specific data.
    ///
    /// This is a pointer that different collectors can use however they see
    /// fit.
    #[inline]
    pub const fn vmctx_gc_heap_data(&self) -> u8 {
        self.vmctx_gc_heap_bound() + self.ptr_size()
    }

    /// The offset of the `type_ids` array pointer.
    #[inline]
    pub const fn vmctx_type_ids_array(&self) -> u8 {
        self.vmctx_gc_heap_data() + self.ptr_size()
    }

    /// The end of statically known offsets in `VMContext`.
    ///
    /// Data after this is dynamically sized.
    #[inline]
    pub const fn vmctx_dynamic_data_start(&self) -> u8 {
        self.vmctx_type_ids_array() + self.ptr_size()
    }
}

impl VMShape {
    pub fn for_module(ptr_size: u8, module: &TranslatedModule) -> Self {
        assert_eq!(ptr_size, StaticVMShape.ptr_size());

        let num_owned_memories = module
            .memories
            .iter()
            .skip(module.num_imported_memories as usize)
            .filter(|p| !p.1.shared)
            .count()
            .try_into()
            .unwrap();

        let mut ret = Self {
            num_imported_functions: module.num_imported_functions,
            num_imported_tables: module.num_imported_tables,
            num_imported_memories: module.num_imported_memories,
            num_imported_globals: module.num_imported_globals,
            num_imported_tags: module.num_imported_tags,
            num_defined_tables: module.num_defined_tables(),
            num_defined_memories: module.num_defined_memories(),
            num_owned_memories,
            num_defined_globals: module.num_defined_globals(),
            num_defined_tags: module.num_defined_tags(),
            num_escaped_funcs: module.num_escaped_functions,
            imported_functions: 0,
            imported_tables: 0,
            imported_memories: 0,
            imported_globals: 0,
            imported_tags: 0,
            defined_tables: 0,
            defined_memories: 0,
            owned_memories: 0,
            defined_globals: 0,
            defined_tags: 0,
            defined_func_refs: 0,
            size: 0,
        };

        // Convenience functions for checked addition and multiplication.
        // As side effect this reduces binary size by using only a single
        // `#[track_caller]` location for each function instead of one for
        // each individual invocation.
        #[inline]
        fn cadd(count: u32, size: u32) -> u32 {
            count.checked_add(size).unwrap()
        }

        #[inline]
        fn cmul(count: u32, size: u8) -> u32 {
            count.checked_mul(u32::from(size)).unwrap()
        }

        /// Align an offset used in this module to a specific byte-width by rounding up
        #[inline]
        fn align(offset: u32, width: u32) -> u32 {
            offset.div_ceil(width) * width
        }

        let mut next_field_offset = u32::from(StaticVMShape.vmctx_dynamic_data_start());

        macro_rules! fields {
            (size($field:ident) = $size:expr, $($rest:tt)*) => {
                ret.$field = next_field_offset;
                next_field_offset = cadd(next_field_offset, u32::from($size));
                fields!($($rest)*);
            };
            (align($align:expr), $($rest:tt)*) => {
                next_field_offset = align(next_field_offset, $align);
                fields!($($rest)*);
            };
            () => {};
        }

        fields! {
            size(imported_memories)
                = cmul(ret.num_imported_memories, u8_size_of::<VMMemoryImport>()),
            size(defined_memories)
                = cmul(ret.num_defined_memories, u8_size_of::<VmPtr<VMMemoryDefinition>>()),
            size(owned_memories)
                = cmul(ret.num_owned_memories, u8_size_of::<VMMemoryDefinition>()),
            size(imported_functions)
                = cmul(ret.num_imported_functions, u8_size_of::<VMFunctionImport>()),
            size(imported_tables)
                = cmul(ret.num_imported_tables, u8_size_of::<VMTableImport>()),
            size(imported_globals)
                = cmul(ret.num_imported_globals, u8_size_of::<VMGlobalImport>()),
            size(imported_tags)
                = cmul(ret.num_imported_tags, u8_size_of::<VMTagImport>()),
            size(defined_tables)
                = cmul(ret.num_defined_tables, u8_size_of::<VMTableDefinition>()),
            align(16),
            size(defined_globals)
                = cmul(ret.num_defined_globals, u8_size_of::<VMGlobalDefinition>()),
            size(defined_tags)
                = cmul(ret.num_defined_tags, u8_size_of::<VMTagDefinition>()),
            size(defined_func_refs) = cmul(
                ret.num_escaped_funcs,
                u8_size_of::<VMFuncRef>()
            ),
        }

        ret.size = next_field_offset;

        ret
    }

    /// Return the offset to the `magic` value in this `VMContext`.
    #[inline]
    pub const fn vmctx_magic(&self) -> u8 {
        StaticVMShape.vmctx_magic()
    }

    /// Return the offset to the `VMStoreContext` structure
    #[inline]
    pub const fn vmctx_store_context(&self) -> u8 {
        StaticVMShape.vmctx_store_context()
    }

    /// Return the offset to the `VMBuiltinFunctionsArray` structure
    #[inline]
    pub const fn vmctx_builtin_functions(&self) -> u8 {
        StaticVMShape.vmctx_builtin_functions()
    }

    /// Return the offset to the `callee` member in this `VMContext`.
    #[inline]
    pub const fn vmctx_callee(&self) -> u8 {
        StaticVMShape.vmctx_callee()
    }

    /// Return the offset to the `*const AtomicU64` epoch-counter
    /// pointer.
    #[inline]
    pub const fn vmctx_epoch_ptr(&self) -> u8 {
        StaticVMShape.vmctx_epoch_ptr()
    }

    /// Return the offset to the GC heap base in this `VMContext`.
    #[inline]
    pub const fn vmctx_gc_heap_base(&self) -> u8 {
        StaticVMShape.vmctx_gc_heap_base()
    }

    /// Return the offset to the GC heap bound in this `VMContext`.
    #[inline]
    pub const fn vmctx_gc_heap_bound(&self) -> u8 {
        StaticVMShape.vmctx_gc_heap_bound()
    }

    /// Return the offset to the `*mut T` collector-specific data.
    ///
    /// This is a pointer that different collectors can use however they see
    /// fit.
    #[inline]
    pub const fn vmctx_gc_heap_data(&self) -> u8 {
        StaticVMShape.vmctx_gc_heap_data()
    }

    /// The offset of the `type_ids` array pointer.
    #[inline]
    pub const fn vmctx_type_ids_array(&self) -> u8 {
        StaticVMShape.vmctx_type_ids_array()
    }

    /// The end of statically known offsets in `VMContext`.
    ///
    /// Data after this is dynamically sized.
    #[inline]
    pub const fn vmctx_dynamic_data_start(&self) -> u8 {
        StaticVMShape.vmctx_dynamic_data_start()
    }

    /// The offset of the `imported_functions` array.
    #[inline]
    pub fn vmctx_imported_functions_begin(&self) -> u32 {
        self.imported_functions
    }

    /// The offset of the `imported_tables` array.
    #[inline]
    pub fn vmctx_imported_tables_begin(&self) -> u32 {
        self.imported_tables
    }

    /// The offset of the `imported_memories` array.
    #[inline]
    pub fn vmctx_imported_memories_begin(&self) -> u32 {
        self.imported_memories
    }

    /// The offset of the `imported_globals` array.
    #[inline]
    pub fn vmctx_imported_globals_begin(&self) -> u32 {
        self.imported_globals
    }

    /// The offset of the `imported_tags` array.
    #[inline]
    pub fn vmctx_imported_tags_begin(&self) -> u32 {
        self.imported_tags
    }

    /// The offset of the `tables` array.
    #[inline]
    pub fn vmctx_tables_begin(&self) -> u32 {
        self.defined_tables
    }

    /// The offset of the `memories` array.
    #[inline]
    pub fn vmctx_memories_begin(&self) -> u32 {
        self.defined_memories
    }

    /// The offset of the `owned_memories` array.
    #[inline]
    pub fn vmctx_owned_memories_begin(&self) -> u32 {
        self.owned_memories
    }

    /// The offset of the `globals` array.
    #[inline]
    pub fn vmctx_globals_begin(&self) -> u32 {
        self.defined_globals
    }

    /// The offset of the `tags` array.
    #[inline]
    pub fn vmctx_tags_begin(&self) -> u32 {
        self.defined_tags
    }

    /// The offset of the `func_refs` array.
    #[inline]
    pub fn vmctx_func_refs_begin(&self) -> u32 {
        self.defined_func_refs
    }

    #[inline]
    pub fn vmctx_vmfunction_import(&self, index: FuncIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_functions);
        self.vmctx_imported_functions_begin()
            + index.as_u32() * u32::from(u8_size_of::<VMFunctionImport>())
    }

    #[inline]
    pub fn vmctx_vmtable_import(&self, index: TableIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_tables);
        self.vmctx_imported_tables_begin()
            + index.as_u32() * u32::from(u8_size_of::<VMTableImport>())
    }

    #[inline]
    pub fn vmctx_vmmemory_import(&self, index: MemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_memories);
        self.vmctx_imported_memories_begin()
            + index.as_u32() * u32::from(u8_size_of::<VMMemoryImport>())
    }

    #[inline]
    pub fn vmctx_vmglobal_import(&self, index: GlobalIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_globals);
        self.vmctx_imported_globals_begin()
            + index.as_u32() * u32::from(u8_size_of::<VMGlobalImport>())
    }

    #[inline]
    pub fn vmctx_vmtag_import(&self, index: TagIndex) -> u32 {
        assert!(index.as_u32() < self.num_imported_tags);
        self.vmctx_imported_tags_begin() + index.as_u32() * u32::from(u8_size_of::<VMTagImport>())
    }

    #[inline]
    pub fn vmctx_vmtable_definition(&self, index: DefinedTableIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_tables);
        self.vmctx_tables_begin() + index.as_u32() * u32::from(u8_size_of::<VMTableDefinition>())
    }

    #[inline]
    pub fn vmctx_vmmemory_pointer(&self, index: DefinedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.num_defined_memories);
        self.vmctx_memories_begin()
            + index.as_u32() * u32::from(u8_size_of::<VmPtr<VMMemoryDefinition>>())
    }

    #[inline]
    pub fn vmctx_vmmemory_definition(&self, index: OwnedMemoryIndex) -> u32 {
        assert!(index.as_u32() < self.owned_memories);
        self.vmctx_owned_memories_begin()
            + index.as_u32() * u32::from(u8_size_of::<VMMemoryDefinition>())
    }

    #[inline]
    pub fn vmctx_vmglobal_definition(&self, index: DefinedGlobalIndex) -> u32 {
        assert!(index.as_u32() < self.defined_globals);
        self.vmctx_globals_begin() + index.as_u32() * u32::from(u8_size_of::<VMGlobalDefinition>())
    }

    #[inline]
    pub fn vmctx_vmtag_definition(&self, index: DefinedTagIndex) -> u32 {
        assert!(index.as_u32() < self.defined_tags);
        self.vmctx_tags_begin() + index.as_u32() * u32::from(u8_size_of::<VMTagDefinition>())
    }

    #[inline]
    pub fn vmctx_vmfunc_ref(&self, index: FuncRefIndex) -> u32 {
        assert!(index.as_u32() < self.defined_func_refs);
        self.vmctx_func_refs_begin() + index.as_u32() * u32::from(u8_size_of::<VMFuncRef>())
    }

    #[inline]
    pub fn size_of_vmctx(&self) -> u32 {
        self.size
    }
}
