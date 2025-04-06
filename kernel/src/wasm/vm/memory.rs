// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::vm::VMMemoryDefinition;

pub struct Memory {
    // /// The underlying allocation backing this memory
    // mem: Vec<u8, UserAllocator>,
}

impl Memory {
    pub(crate) fn vmmemory_definition(&self) -> VMMemoryDefinition {
        todo!()
    }
}