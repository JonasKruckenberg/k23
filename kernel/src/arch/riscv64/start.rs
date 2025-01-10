// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::STACK_SIZE_PAGES;
use core::arch::naked_asm;
use loader_api::LoaderConfig;
use mmu::arch::PAGE_SIZE;

#[unsafe(link_section = ".bss.uninit")]
pub static BOOT_STACK: Stack = Stack([0; STACK_SIZE_PAGES * PAGE_SIZE]);

#[repr(C, align(128))]
pub struct Stack(pub [u8; STACK_SIZE_PAGES * PAGE_SIZE]);

#[used(linker)]
#[unsafe(link_section = ".loader_config")]
static LOADER_CONFIG: LoaderConfig = LoaderConfig::new_default();

#[unsafe(no_mangle)]
#[naked]
unsafe extern "C" fn _start(hartid: usize, boot_info: &'static loader_api::BootInfo) -> ! {
    unsafe {
        naked_asm! {
            // Setup the stack pointer
            "la   t0, {boot_stack_start}",  // set the stack pointer to the bottom of the stack
            "li   t1, {boot_stack_size}",   // load the stack size
            "add  sp, t0, t1",              // add both to get the top of the stack

            // Fill the stack with a canary pattern (0xACE0BACE) so that we can identify unused stack memory
            // in dumps & calculate stack usage. This is also really great (don't ask my why I know this) to identify
            // when we tried executing stack memory.
            "li          t1, 0xACE0BACE",
            "1:",
            "   sw          t1, 0(t0)",     // write the canary as u64
            "   addi        t0, t0, 8",     // move to the next u64
            "   bltu        t0, sp, 1b",    // loop until we reach the top of the stack

            // Call the rust entry point
            "call {start_rust}",

            // Loop forever.
            // `start_rust` should never return, but in case it does prevent the hart from executing
            // random code
            "2:",
            "   wfi",
            "   j 2b",

            boot_stack_start = sym BOOT_STACK,
            boot_stack_size = const STACK_SIZE_PAGES * PAGE_SIZE,
            start_rust = sym crate::main,
        }
    }
}
