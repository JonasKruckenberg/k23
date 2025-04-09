// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::mem::Mmap;
use crate::wasm::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::wasm::module::Module;
use crate::wasm::translate::TranslatedModule;
use crate::wasm::utils::round_usize_up_to_host_pages;
use crate::wasm::vm::instance::Instance;
use crate::wasm::vm::mmap_vec::MmapVec;
use crate::wasm::vm::{InstanceHandle, TableElement, VMShape};
use crate::wasm::{MEMORY_MAX, TABLE_MAX, translate, vm};
use anyhow::Context;
use core::alloc::Allocator;
use core::ptr::NonNull;
use core::{cmp, mem};
use cranelift_entity::PrimaryMap;

/// A type that knows how to allocate backing memory for instance resources.
pub trait InstanceAllocator {
    unsafe fn allocate_instance_and_vmctx(
        &self,
        vmshape: &VMShape,
    ) -> crate::Result<NonNull<Instance>>;
    unsafe fn deallocate_instance_and_vmctx(&self, instance: NonNull<Instance>, vmshape: &VMShape);

    /// Allocate a memory for an instance.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the allocations fail.
    ///
    /// # Safety
    ///
    /// The safety of the entire VM depends on the correct implementation of this method.
    unsafe fn allocate_memory(
        &self,
        memory: &translate::Memory,
        memory_index: DefinedMemoryIndex,
    ) -> crate::Result<vm::Memory>;

    /// Deallocate an instance's previously allocated memory.
    ///
    /// # Safety
    ///
    /// The memory must have previously been allocated by
    /// `Self::allocate_memory`, be at the given index, and must currently be
    /// allocated. It must never be used again.
    unsafe fn deallocate_memory(&self, memory_index: DefinedMemoryIndex, memory: vm::Memory);

    // fn allocate_fiber_stack(&self) -> crate::Result<FiberStack>;
    // unsafe fn deallocate_fiber_stack(&self, stack: FiberStack);

    /// Allocate a table for an instance.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the allocations fail.
    ///
    /// # Safety
    ///
    /// The safety of the entire VM depends on the correct implementation of this method.
    unsafe fn allocate_table(
        &self,
        table_desc: &translate::Table,
        table_index: DefinedTableIndex,
    ) -> crate::Result<vm::Table>;

    /// Deallocate an instance's previously allocated table.
    ///
    /// # Safety
    ///
    /// The table must have previously been allocated by `Self::allocate_table`,
    /// be at the given index, and must currently be allocated. It must never be
    /// used again.
    unsafe fn deallocate_table(&self, table_index: DefinedTableIndex, table: vm::Table);

    /// Allocate multiple memories at once.
    ///
    /// By default, this will delegate the actual allocation to `Self::allocate_memory`.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the allocations fail.
    ///
    /// # Safety
    ///
    /// The safety of the entire VM depends on the correct implementation of this method.
    unsafe fn allocate_memories(
        &self,
        module: &TranslatedModule,
        memories: &mut PrimaryMap<DefinedMemoryIndex, vm::Memory>,
    ) -> crate::Result<()> {
        for (index, plan) in &module.memories {
            if let Some(def_index) = module.defined_memory_index(index) {
                let new_def_index =
                    // Safety: caller has to ensure safety
                    memories.push(unsafe { self.allocate_memory(plan, def_index)? });
                debug_assert_eq!(def_index, new_def_index);
            }
        }
        Ok(())
    }

    /// Allocate multiple tables at once.
    ///
    /// By default, this will delegate the actual allocation to `Self::allocate_table`.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the allocations fail.
    ///
    /// # Safety
    ///
    /// The safety of the entire VM depends on the correct implementation of this method.
    unsafe fn allocate_tables(
        &self,
        module: &TranslatedModule,
        tables: &mut PrimaryMap<DefinedTableIndex, vm::Table>,
    ) -> crate::Result<()> {
        for (index, plan) in &module.tables {
            if let Some(def_index) = module.defined_table_index(index) {
                let new_def_index =
                    // Safety: caller has to ensure safety
                    tables.push(unsafe { self.allocate_table(plan, def_index)? });
                debug_assert_eq!(def_index, new_def_index);
            }
        }
        Ok(())
    }

    /// Deallocates multiple memories at once.
    ///
    /// By default, this will delegate the actual deallocation to `Self::deallocate_memory`.
    ///
    /// # Safety
    ///
    /// Just like `Self::deallocate_memory` all memories must have been allocated by
    /// `Self::allocate_memories`/`Self::allocate_memory` and must never be used again.
    unsafe fn deallocate_memories(
        &self,
        memories: &mut PrimaryMap<DefinedMemoryIndex, vm::Memory>,
    ) {
        for (memory_index, memory) in mem::take(memories) {
            // Because deallocating memory is infallible, we don't need to worry
            // about leaking subsequent memories if the first memory failed to
            // deallocate. If deallocating memory ever becomes fallible, we will
            // need to be careful here!
            // Safety: ensured by caller
            unsafe {
                self.deallocate_memory(memory_index, memory);
            }
        }
    }

    /// Deallocates multiple tables at once.
    ///
    /// By default, this will delegate the actual deallocation to `Self::deallocate_table`.
    ///
    /// # Safety
    ///
    /// Just like `Self::deallocate_table` all tables must have been allocated by
    /// `Self::allocate_tables`/`Self::allocate_table` and must never be used again.
    unsafe fn deallocate_tables(&self, tables: &mut PrimaryMap<DefinedTableIndex, vm::Table>) {
        for (table_index, table) in mem::take(tables) {
            // Safety: ensured by caller
            unsafe {
                self.deallocate_table(table_index, table);
            }
        }
    }

    /// Allocate all resources required to instantiate a module.
    ///
    /// By default, this will in-turn call `Self::allocate_vmctx`, `Self::allocate_tables` and
    /// `Self::allocate_memories` as well as perform necessary clean up.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the allocations fail. In this case, the resources are cleaned up
    /// automatically.
    fn allocate_module(&self, module: Module) -> crate::Result<InstanceHandle> {
        let mut tables = PrimaryMap::with_capacity(
            usize::try_from(module.translated().num_defined_tables()).unwrap(),
        );
        let mut memories = PrimaryMap::with_capacity(
            usize::try_from(module.translated().num_defined_memories()).unwrap(),
        );

        // Safety: TODO
        match (|| unsafe {
            self.allocate_tables(module.translated(), &mut tables)?;
            self.allocate_memories(module.translated(), &mut memories)?;
            self.allocate_instance_and_vmctx(module.vmshape())
        })() {
            // Safety: we crated the instance handle and memories/tables from the same module description so should be fine
            Ok(instance) => Ok(unsafe { Instance::from_parts(module, instance, tables, memories) }),
            // Safety: memories and tables have just been allocated and will not be handed out
            Err(e) => unsafe {
                self.deallocate_memories(&mut memories);
                self.deallocate_tables(&mut tables);
                Err(e)
            },
        }
    }

    unsafe fn deallocate_module(&self, handle: &mut InstanceHandle) {
        // Safety: ensured by caller
        unsafe {
            self.deallocate_memories(&mut handle.instance_mut().memories);
            self.deallocate_tables(&mut handle.instance_mut().tables);
            self.deallocate_instance_and_vmctx(handle.as_non_null(), handle.instance().vmshape());
        }
    }
}

pub struct PlaceholderAllocatorDontUse;
impl InstanceAllocator for PlaceholderAllocatorDontUse {
    unsafe fn allocate_instance_and_vmctx(
        &self,
        vmshape: &VMShape,
    ) -> crate::Result<NonNull<Instance>> {
        // FIXME this shouldn't allocate from the kernel heap
        let ptr = alloc::alloc::Global.allocate(Instance::alloc_layout(vmshape))?;
        Ok(ptr.cast())
    }

    unsafe fn deallocate_instance_and_vmctx(&self, instance: NonNull<Instance>, vmshape: &VMShape) {
        // Safety: `NonNull<Instance>` is only ever created above using the same global allocator
        unsafe {
            // FIXME this shouldn't allocate from the kernel heap
            let layout = Instance::alloc_layout(vmshape);
            alloc::alloc::Global.deallocate(instance.cast(), layout);
        }
    }

    unsafe fn allocate_memory(
        &self,
        memory: &translate::Memory,
        _memory_index: DefinedMemoryIndex,
    ) -> crate::Result<vm::Memory> {
        // TODO we could call out to some resource management instance here to obtain
        //  dynamic "minimum" and "maximum" values that reflect the state of the real systems
        //  memory consumption

        // If the minimum memory size overflows the size of our own address
        // space, then we can't satisfy this request, but defer the error to
        // later so the `store` can be informed that an effective oom is
        // happening.
        let minimum = memory
            .minimum_byte_size()
            .ok()
            .and_then(|m| usize::try_from(m).ok())
            .expect("memory minimum size exceeds memory limits");

        // The plan stores the maximum size in units of wasm pages, but we
        // use units of bytes. Unlike for the `minimum` size we silently clamp
        // the effective maximum size to the limits of what we can track. If the
        // maximum size exceeds `usize` or `u64` then there's no need to further
        // keep track of it as some sort of runtime limit will kick in long
        // before we reach the statically declared maximum size.
        let maximum = memory
            .maximum_byte_size()
            .ok()
            .and_then(|m| usize::try_from(m).ok());

        let offset_guard_bytes = usize::try_from(memory.offset_guard_size).unwrap();
        // Ensure that our guard regions are multiples of the host page size.
        let offset_guard_bytes = round_usize_up_to_host_pages(offset_guard_bytes);

        let bound_bytes = round_usize_up_to_host_pages(MEMORY_MAX);
        let allocation_bytes = bound_bytes.min(maximum.unwrap_or(usize::MAX));
        let request_bytes = allocation_bytes + offset_guard_bytes;

        let mmap = crate::mem::with_kernel_aspace(|aspace| {
            // attempt to use 2MiB alignment but if that's not available fallback to the largest
            let align = cmp::min(2 * 1048576, aspace.frame_alloc.max_alignment());

            // TODO the align arg should be a named const not a weird number like this
            Mmap::new_zeroed(aspace, request_bytes, align, None)
                .context("Failed to mmap zeroed memory for Memory")
        })?;

        Ok(vm::Memory::from_parts(
            mmap,
            minimum,
            maximum,
            memory.page_size_log2,
            offset_guard_bytes,
        ))
    }

    unsafe fn deallocate_memory(&self, _memory_index: DefinedMemoryIndex, _memory: vm::Memory) {}

    unsafe fn allocate_table(
        &self,
        table: &translate::Table,
        _table_index: DefinedTableIndex,
    ) -> crate::Result<vm::Table> {
        // TODO we could call out to some resource management instance here to obtain
        //  dynamic "minimum" and "maximum" values that reflect the state of the real systems
        //  memory consumption
        let maximum = table.limits.max.and_then(|m| usize::try_from(m).ok());
        let reserve_size = TABLE_MAX.min(maximum.unwrap_or(usize::MAX));

        let elements = if reserve_size == 0 {
            MmapVec::new_empty()
        } else {
            crate::mem::with_kernel_aspace(|aspace| -> crate::Result<_> {
                let mut elements = MmapVec::new_zeroed(aspace, reserve_size)?;
                elements.extend_with(
                    aspace,
                    usize::try_from(table.limits.min).unwrap(),
                    TableElement::FuncRef(None),
                );
                Ok(elements)
            })?
        };

        Ok(vm::Table::from_parts(elements, maximum))
    }

    unsafe fn deallocate_table(&self, _table_index: DefinedTableIndex, _table: vm::Table) {}
}
