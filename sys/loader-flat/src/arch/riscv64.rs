// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::arch::naked_asm;

pub const PAGE_SIZE: usize = 4096;

/// 16-byte-aligned newtype so the stack top (`&BOOT_STACK as *const _ + STACK_SIZE`) is also
/// 16-byte aligned, satisfying the RISC-V ABI stack-alignment requirement.
#[repr(C, align(16))]
struct BootStack([u8; crate::STACK_SIZE]);

/// Boot-hart stack.  Zero-initialised so it lands in BSS, but placed in `.bss.uninit` — a
/// section intentionally outside `__bss_start`/`__bss_end` — so the BSS-clear loop in `_start`
/// cannot stomp on a stack that is already in use.
#[unsafe(link_section = ".bss.uninit")]
static BOOT_STACK: BootStack = BootStack([0u8; crate::STACK_SIZE]);

/// Image entry point, invoked by the prior boot stage (QEMU's `-kernel` loader / SBI
/// firmware).
///
/// # Safety
///
/// The RISC-V firmware must uphold the following boot contract:
///
/// - executing in S-mode on the boot hart, with this function at the image entry PC,
/// - `a0` = boot hart ID, `a1` = physical address of the DTB (Linux/SBI convention),
/// - the `.bss` and boot stack this prologue initializes have not yet been used.
#[unsafe(link_section = ".text.start")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
unsafe extern "C" fn _start() -> ! {
    naked_asm! {
        // FIXME this is a workaround for bug in rustc/llvm
        //  https://github.com/rust-lang/rust/issues/80608#issuecomment-1094267279
        ".attribute arch, \"rv64gc\"",

        // read boot time stamp as early as possible
        "rdtime a2",

        // Clear return address and frame pointer
        "mv     ra, zero",
        "mv     s0, zero",

        // Clear the gp register in case anything tries to use it.
        "mv     gp, zero",

        // Mask all interrupts in case the previous stage left them on.
        "csrc   sstatus, 1 << 1",
        "csrw   sie, zero",

        // Reset the trap vector in case the previous stage left one installed.
        "csrw   stvec, zero",

        // Disable the MMU in case it was left on.
        "csrw   satp, zero",

        // Setup the stack pointer
        "la     t0, {boot_stack}",  // t0 = bottom of boot stack
        "li     t1, {stack_size}",  // load the stack size
        "add    sp, t0, t1",        // sp = stack top

        // fill stack with canary pattern
        // $sp is set to stack top above, $t0 as well
        "call   {fill_stack}",

        // Clear .bss.  The linker script ensures these are aligned to 16 bytes.
        // .bss.uninit (where BOOT_STACK lives) is outside these bounds on purpose.
        "lla    a3, __bss_start",
        "lla    a4, __bss_end",
        "0:",
        "   sd      zero, (a3)",
        "   sd      zero, 8(a3)",
        "   add     a3, a3, 16",
        "   blt     a3, a4, 0b",

        // Call the rust entry point
        "call {start_rust}",

        // Loop forever.
        // `start_rust` should never return, but in case it does prevent the hart from executing
        // random code
        "2:",
        "   wfi",
        "   j 2b",

        boot_stack  = sym BOOT_STACK,
        stack_size  = const crate::STACK_SIZE,
        start_rust  = sym crate::main,
        fill_stack  = sym fill_stack
    }
}

/// Fill the stack with a canary pattern (0xACE0BACE) so that we can identify unused stack memory
/// in dumps & calculate stack usage. This is also really great (don't ask my why I know this) to identify
/// when we tried executing stack memory.
///
/// # Safety
///
/// expects the bottom of the stack in `t0` and the top of stack in `sp`
#[unsafe(naked)]
unsafe extern "C" fn fill_stack() {
    naked_asm! {
        // Fill the stack with a canary pattern (0xACE0BACE) so that we can identify unused stack memory
        // in dumps & calculate stack usage. This is also really great (don't ask my why I know this) to identify
        // when we tried executing stack memory.
        "li     t1, 0xACE0BACEACE0BACE",
        "1:",
        "   sd          t1, 0(t0)",     // write the canary as u64
        "   addi        t0, t0, 8",     // move to the next u64
        "   bltu        t0, sp, 1b",    // loop until we reach the top of the stack
        "ret"
    }
}
