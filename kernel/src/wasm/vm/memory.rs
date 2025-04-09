// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::mem::{Mmap, VirtualAddress};
use crate::wasm::vm::VMMemoryDefinition;
use crate::wasm::vm::provenance::VmPtr;
use core::ptr::NonNull;
use core::range::Range;

#[derive(Debug)]
pub struct Memory {
    /// The underlying allocation backing this memory
    mmap: Mmap,
    // mem: Vec<u8, UserAllocator>,
    /// The current length of this Wasm memory, in bytes.
    len: usize,
    /// The optional maximum accessible size, in bytes, for this linear memory.
    ///
    /// This **does not** include guard pages and might be smaller than `self.accessible`
    /// since the underlying allocation is always a multiple of the host page size.
    maximum: Option<usize>,
    /// The log2 of this Wasm memory's page size, in bytes.
    page_size_log2: u8,
    /// Size in bytes of extra guard pages after the end to
    /// optimize loads and stores with constant offsets.
    offset_guard_size: usize,
}

impl Memory {
    pub(crate) fn from_parts(
        mmap: Mmap,
        len: usize,
        maximum: Option<usize>,
        page_size_log2: u8,
        offset_guard_size: usize,
    ) -> Self {
        Self {
            mmap,
            len,
            maximum,
            page_size_log2,
            offset_guard_size,
        }
    }

    pub fn byte_size(&self) -> usize {
        self.len
    }

    pub fn wasm_accessible(&self) -> Range<VirtualAddress> {
        self.mmap.range()
    }

    // /// Implementation of `memory.atomic.notify` for all memories.
    // pub fn atomic_notify(&mut self, addr: u64, count: u32) -> Result<u32, Trap> {
    //     match self.as_shared_memory() {
    //         Some(m) => m.atomic_notify(addr, count),
    //         None => {
    //             validate_atomic_addr(&self.vmmemory(), addr, 4, 4)?;
    //             Ok(0)
    //         }
    //     }
    // }
    //
    // /// Implementation of `memory.atomic.wait32` for all memories.
    // pub fn atomic_wait32(
    //     &mut self,
    //     addr: u64,
    //     expected: u32,
    //     timeout: Option<Duration>,
    // ) -> Result<WaitResult, Trap> {
    //     match self.as_shared_memory() {
    //         Some(m) => m.atomic_wait32(addr, expected, timeout),
    //         None => {
    //             validate_atomic_addr(&self.vmmemory(), addr, 4, 4)?;
    //             Err(Trap::AtomicWaitNonSharedMemory)
    //         }
    //     }
    // }
    //
    // /// Implementation of `memory.atomic.wait64` for all memories.
    // pub fn atomic_wait64(
    //     &mut self,
    //     addr: u64,
    //     expected: u64,
    //     timeout: Option<Duration>,
    // ) -> Result<WaitResult, Trap> {
    //     match self.as_shared_memory() {
    //         Some(m) => m.atomic_wait64(addr, expected, timeout),
    //         None => {
    //             validate_atomic_addr(&self.vmmemory(), addr, 8, 8)?;
    //             Err(Trap::AtomicWaitNonSharedMemory)
    //         }
    //     }
    // }

    pub(crate) fn vmmemory_definition(&mut self) -> VMMemoryDefinition {
        VMMemoryDefinition {
            base: VmPtr::from(NonNull::new(self.mmap.as_mut_ptr()).unwrap()),
            current_length: self.len.into(),
        }
    }
}
