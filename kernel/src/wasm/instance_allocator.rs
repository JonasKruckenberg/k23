use crate::vm::{AddressSpace};
use crate::wasm::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::wasm::runtime::{InstanceAllocator, Memory, Table};
use crate::wasm::runtime::{OwnedVMContext, VMOffsets};
use crate::wasm::translate::{MemoryDesc, TableDesc, TranslatedModule};

/// A placeholder allocator impl that just delegates to runtime types `new` methods.
pub struct PlaceholderAllocatorDontUse;

impl InstanceAllocator for PlaceholderAllocatorDontUse {
    unsafe fn allocate_vmctx(
        &self,
        aspace: &mut AddressSpace,
        _module: &TranslatedModule,
        plan: &VMOffsets,
    ) -> crate::wasm::Result<OwnedVMContext> {
        OwnedVMContext::try_new(aspace, plan)
    }

    unsafe fn deallocate_vmctx(&self, _aspace: &mut AddressSpace, _vmctx: OwnedVMContext) {}

    unsafe fn allocate_memory(
        &self,
        aspace: &mut AddressSpace,
        _module: &TranslatedModule,
        memory_desc: &MemoryDesc,
        _memory_index: DefinedMemoryIndex,
    ) -> crate::wasm::Result<Memory> {
        // TODO we could call out to some resource management instance here to obtain
        // dynamic "minimum" and "maximum" values that reflect the state of the real systems
        // memory consumption

        // If the minimum memory size overflows the size of our own address
        // space, then we can't satisfy this request, but defer the error to
        // later so the `store` can be informed that an effective oom is
        // happening.
        let minimum = memory_desc
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
        let maximum = memory_desc
            .maximum_byte_size()
            .ok()
            .and_then(|m| usize::try_from(m).ok());

        Memory::try_new(aspace, memory_desc, minimum, maximum)
    }

    unsafe fn deallocate_memory(
        &self,
        _aspace: &mut AddressSpace,
        _memory_index: DefinedMemoryIndex,
        _memory: Memory,
    ) {
    }

    unsafe fn allocate_table(
        &self,
        aspace: &mut AddressSpace,
        _module: &TranslatedModule,
        table_desc: &TableDesc,
        _table_index: DefinedTableIndex,
    ) -> crate::wasm::Result<Table> {
        // TODO we could call out to some resource management instance here to obtain
        // dynamic "minimum" and "maximum" values that reflect the state of the real systems
        // memory consumption
        let maximum = table_desc.maximum.and_then(|m| usize::try_from(m).ok());

        Table::try_new(aspace, table_desc, maximum)
    }

    unsafe fn deallocate_table(
        &self,
        _aspace: &mut AddressSpace,
        _table_index: DefinedTableIndex,
        _table: Table,
    ) {
    }
}
