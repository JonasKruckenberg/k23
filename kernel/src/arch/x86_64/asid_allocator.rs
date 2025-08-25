// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// x86_64 doesn't have ASIDs like RISC-V, but this provides a compatible interface
#[derive(Debug)]
pub struct AsidAllocator {
    // TODO: x86_64 might use PCID (Process Context Identifiers) instead
}

impl AsidAllocator {
    pub fn new() -> Self {
        Self {}
    }

    pub fn alloc(&mut self) -> Option<u16> {
        // TODO: Implement PCID allocation if supported
        Some(0)
    }

    pub fn free(&mut self, _asid: u16) {
        // TODO: Implement PCID deallocation
    }
}

#[cold]
pub fn init() {
    // TODO: Initialize PCID support if available
}
