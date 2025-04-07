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
    
    pub(crate) fn vmmemory_definition(&self) -> VMMemoryDefinition {
        todo!()
    }
}