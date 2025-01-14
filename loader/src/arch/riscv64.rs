// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::arch::{asm, naked_asm};
use loader_api::BootInfo;

pub const KERNEL_ASID: usize = 0;
pub const KERNEL_ASPACE_BASE: usize = 0xffffffc000000000;
pub const PAGE_SIZE: usize = 4096;
/// The number of page table entries in one table
pub const PAGE_TABLE_ENTRIES: usize = 512;
pub const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2 Sv39
pub const VIRT_ADDR_BITS: u32 = 38;

pub const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;
pub const PAGE_ENTRY_SHIFT: usize = (PAGE_TABLE_ENTRIES - 1).count_ones() as usize;

const BOOT_STACK_SIZE: usize = 32 * PAGE_SIZE;

#[unsafe(link_section = ".bss.uninit")]
static BOOT_STACK: Stack = Stack([0; BOOT_STACK_SIZE]);

#[repr(C, align(128))]
struct Stack([u8; BOOT_STACK_SIZE]);

#[unsafe(link_section = ".text.start")]
#[unsafe(no_mangle)]
#[naked]
unsafe extern "C" fn _start() -> ! {
    unsafe {
        naked_asm! {
            // read boot time stamp as early as possible
            "rdtime a2",

            // Clear return address and frame pointer
            "mv ra, zero",
            "mv s0, zero",

            // Clear the gp register in case anything tries to use it.
            "mv gp, zero",

            // Mask all interrupts in case the previous stage left them on.
            "csrc sstatus, 1 << 1",
            "csrw sie, zero",

            // Reset the trap vector in case the previous stage left one installed.
            "csrw stvec, zero",

            // Disable the MMU in case it was left on.
            "csrw satp, zero",

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
            boot_stack_size = const BOOT_STACK_SIZE,
            start_rust = sym crate::main,
        }
    }
}

pub unsafe fn handoff_to_kernel(hartid: usize, boot_info: *mut BootInfo, entry: usize) -> ! {
    log::debug!("Hart {hartid} Jumping to kernel...");
    log::trace!("Hart {hartid} entry: {entry}, arguments: a0={hartid} a1={boot_info:?}");

    unsafe {
        asm! {
            "mv ra, zero", // Reset return address

            "jalr zero, {kernel_entry}",

            // Loop forever.
            // The kernel should never return, but in case it does prevent the hart from executing
            // random code
            "1:",
            "   wfi",
            "   j 1b",
            in("a0") hartid,
            in("a1") boot_info,
            kernel_entry = in(reg) entry,
            options(noreturn)
        }
    }
}

/// Return the page size for the given page table level.
///
/// # Panics
///
/// Panics if the provided level is `>= PAGE_TABLE_LEVELS`.
pub fn page_size_for_level(level: usize) -> usize {
    assert!(level < PAGE_TABLE_LEVELS);
    let page_size = 1 << (PAGE_SHIFT + level * PAGE_ENTRY_SHIFT);
    debug_assert!(page_size == 4096 || page_size == 2097152 || page_size == 1073741824);
    page_size
}
