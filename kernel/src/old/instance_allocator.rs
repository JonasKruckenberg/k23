// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::fiber::FiberStack;
use crate::mem::AddressSpace;
use crate::mem::frame_alloc::FrameAllocator;
use crate::wasm::Engine;
use crate::wasm::indices::{DefinedMemoryIndex, DefinedTableIndex};
use crate::wasm::runtime::{InstanceAllocator, Memory, Table};
use crate::wasm::runtime::{OwnedVMContext, VMOffsets};
use crate::wasm::translate::{MemoryDesc, TableDesc, TranslatedModule};
use alloc::sync::Arc;
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use spin::Mutex;

/// A placeholder allocator impl that just delegates to runtime types `new` methods.
#[derive(Debug)]
pub struct PlaceholderAllocatorDontUse(pub(super) Arc<Mutex<AddressSpace>>);

impl PlaceholderAllocatorDontUse {
    pub fn new(engine: &Engine, frame_alloc: &'static FrameAllocator) -> Self {
        let aspace = AddressSpace::new_user(
            engine.allocate_asid(),
            engine.rng().map(|mut rng| ChaCha20Rng::from_rng(&mut rng)),
            frame_alloc,
        )
        .unwrap();

        Self(Arc::new(Mutex::new(aspace)))
    }
}

impl InstanceAllocator for PlaceholderAllocatorDontUse {
    unsafe fn allocate_vmctx(
        &self,
        _module: &TranslatedModule,
        plan: &VMOffsets,
    ) -> crate::Result<OwnedVMContext> {
        let mut aspace = self.0.lock();
        OwnedVMContext::try_new(&mut aspace, plan)
    }

    unsafe fn deallocate_vmctx(&self, _vmctx: OwnedVMContext) {}

    unsafe fn allocate_memory(
        &self,
        _module: &TranslatedModule,
        memory_desc: &MemoryDesc,
        _memory_index: DefinedMemoryIndex,
    ) -> crate::Result<Memory> {
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

        let mut aspace = self.0.lock();
        Memory::try_new(&mut aspace, memory_desc, minimum, maximum)
    }

    unsafe fn deallocate_memory(&self, _memory_index: DefinedMemoryIndex, _memory: Memory) {}

    fn allocate_fiber_stack(&self) -> crate::Result<FiberStack> {
        let mut aspace = self.0.lock();
        Ok(FiberStack::new(&mut aspace))
    }

    unsafe fn deallocate_fiber_stack(&self, _stack: FiberStack) {}

    unsafe fn allocate_table(
        &self,
        _module: &TranslatedModule,
        table_desc: &TableDesc,
        _table_index: DefinedTableIndex,
    ) -> crate::Result<Table> {
        // TODO we could call out to some resource management instance here to obtain
        // dynamic "minimum" and "maximum" values that reflect the state of the real systems
        // memory consumption
        let maximum = table_desc.maximum.and_then(|m| usize::try_from(m).ok());

        let mut aspace = self.0.lock();
        Table::try_new(&mut aspace, table_desc, maximum)
    }

    unsafe fn deallocate_table(&self, _table_index: DefinedTableIndex, _table: Table) {}
}
