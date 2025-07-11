// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::VMMemoryDefinition;
use crate::vm::provenance::VmPtr;
use alloc::vec::Vec;
use core::ptr::NonNull;

/// A WebAssembly linear memory instance.
///
/// https://webassembly.github.io/spec/core/exec/runtime.html#memory-instances
#[derive(Debug)]
pub struct Memory {
    /// The underlying allocation backing this memory
    mem: Vec<u8>,
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

// === impl Memory ===

impl Memory {
    pub fn new(
        mem: Vec<u8>,
        maximum: Option<usize>,
        page_size_log2: u8,
        offset_guard_size: usize,
    ) -> Self {
        Self {
            mem,
            maximum,
            page_size_log2,
            offset_guard_size,
        }
    }

    pub(crate) fn vmmemory_definition(&mut self) -> VMMemoryDefinition {
        VMMemoryDefinition {
            base: VmPtr::from(NonNull::new(self.mem.as_mut_ptr()).unwrap()),
            current_length: self.mem.len().into(),
        }
    }
}
