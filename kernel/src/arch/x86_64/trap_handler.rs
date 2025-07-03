// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::TRAP_STACK_SIZE_PAGES;
use crate::arch::PAGE_SIZE;
use core::cell::Cell;
use cpu_local::cpu_local;

cpu_local! {
    static IN_TRAP: Cell<bool> = Cell::new(false);
    static TRAP_STACK: [u8; TRAP_STACK_SIZE_PAGES * PAGE_SIZE] = const { [0; TRAP_STACK_SIZE_PAGES * PAGE_SIZE] };
}

#[cold]
pub fn init() {
    // Ensure TRAP_STACK is not optimized out by referencing it
    let _trap_stack_top = unsafe {
        TRAP_STACK
            .as_ptr()
            .byte_add(TRAP_STACK_SIZE_PAGES * PAGE_SIZE)
            .cast_mut()
    };

    panic!("trap handler initialization not implemented yet");
    // TODO: Initialize x86_64 interrupt descriptor table (IDT)
    // TODO: Set up exception handlers for page faults, general protection faults, etc.
}
