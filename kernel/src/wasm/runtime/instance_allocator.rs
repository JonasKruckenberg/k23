use crate::vm::AddressSpace;
use crate::wasm::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::wasm::runtime::memory::Memory;
use crate::wasm::runtime::table::Table;
use crate::wasm::runtime::{OwnedVMContext, VMOffsets};
use crate::wasm::translate::{MemoryDesc, TableDesc, TranslatedModule};
use crate::wasm::Module;
use core::mem;
use cranelift_entity::PrimaryMap;

/// A type that knows how to allocate backing memory for instance resources.
pub trait InstanceAllocator {
    /// Allocate the `VMContext` for an instance.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the allocations fail.
    ///
    /// # Safety
    ///
    /// The safety of the entire VM depends on the correct implementation of this method.
    unsafe fn allocate_vmctx(
        &self,
        module: &TranslatedModule,
        offsets: &VMOffsets,
    ) -> crate::wasm::Result<OwnedVMContext>;

    /// Deallocate a `VMContext` of an instance.
    ///
    /// # Safety
    ///
    /// The `VMContext` must have previously been allocated by
    /// `Self::allocate_vmctx`
    unsafe fn deallocate_vmctx(&self, vmctx: OwnedVMContext);

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
        module: &TranslatedModule,
        memory_desc: &MemoryDesc,
        memory_index: DefinedMemoryIndex,
    ) -> crate::wasm::Result<Memory>;

    /// Deallocate an instance's previously allocated memory.
    ///
    /// # Safety
    ///
    /// The memory must have previously been allocated by
    /// `Self::allocate_memory`, be at the given index, and must currently be
    /// allocated. It must never be used again.
    unsafe fn deallocate_memory(&self, memory_index: DefinedMemoryIndex, memory: Memory);

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
        module: &TranslatedModule,
        table_desc: &TableDesc,
        table_index: DefinedTableIndex,
    ) -> crate::wasm::Result<Table>;

    /// Deallocate an instance's previously allocated table.
    ///
    /// # Safety
    ///
    /// The table must have previously been allocated by `Self::allocate_table`,
    /// be at the given index, and must currently be allocated. It must never be
    /// used again.
    unsafe fn deallocate_table(&self, table_index: DefinedTableIndex, table: Table);

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
        memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>,
    ) -> crate::wasm::Result<()> {
        for (index, plan) in &module.memories {
            if let Some(def_index) = module.defined_memory_index(index) {
                let new_def_index =
                // Safety: caller has to ensure safety
                    memories.push(unsafe { self.allocate_memory(module, plan, def_index)? });
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
        tables: &mut PrimaryMap<DefinedTableIndex, Table>,
    ) -> crate::wasm::Result<()> {
        for (index, plan) in &module.tables {
            if let Some(def_index) = module.defined_table_index(index) {
                let new_def_index =
                // Safety: caller has to ensure safety
                    tables.push(unsafe { self.allocate_table(module, plan, def_index)? });
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
    unsafe fn deallocate_memories(&self, memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>) {
        for (memory_index, memory) in mem::take(memories) {
            // Because deallocating memory is infallible, we don't need to worry
            // about leaking subsequent memories if the first memory failed to
            // deallocate. If deallocating memory ever becomes fallible, we will
            // need to be careful here!
            // Safety: caller has to ensure safety
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
    unsafe fn deallocate_tables(&self, tables: &mut PrimaryMap<DefinedTableIndex, Table>) {
        for (table_index, table) in mem::take(tables) {
            // Safety: caller has to ensure safety
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
    #[expect(
        clippy::type_complexity,
        reason = "TODO clean up the return type and remove"
    )]
    fn allocate_module(
        &self,
        module: &Module,
    ) -> crate::wasm::Result<(
        OwnedVMContext,
        PrimaryMap<DefinedTableIndex, Table>,
        PrimaryMap<DefinedMemoryIndex, Memory>,
    )> {
        let num_defined_tables =
            module.translated().num_tables() - module.translated().num_imported_tables();
        let mut tables = PrimaryMap::with_capacity(usize::try_from(num_defined_tables).unwrap());

        let num_defined_memories =
            module.translated().num_memories() - module.translated().num_imported_memories();
        let mut memories =
            PrimaryMap::with_capacity(usize::try_from(num_defined_memories).unwrap());
        
        // Safety: TODO
        match (|| unsafe {
            self.allocate_tables(module.translated(), &mut tables)?;
            self.allocate_memories(module.translated(), &mut memories)?;
            self.allocate_vmctx(module.translated(), module.offsets())
        })() {
            Ok(vmctx) => Ok((vmctx, tables, memories)),
            // Safety: memories and tables have just been allocated and will not be handed out
            Err(e) => unsafe {
                self.deallocate_memories(&mut memories);
                self.deallocate_tables(&mut tables);
                Err(e)
            },
        }
    }
}
