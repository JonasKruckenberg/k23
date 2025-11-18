// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::arch::{asm, naked_asm};

use kmem_core::bootstrap::{BootstrapAddressSpace, BootstrapAllocator, BootstrapArch};
use kmem_core::{AllocError, Flush, FrameAllocator};
use riscv::satp;

use crate::error::Error;
use crate::machine_info::MachineInfo;
use crate::GlobalInitResult;

/// Entry point for the initializing hart, this will set up the CPU environment for Rust and then
/// transfer control to [`crate::main`].
///
/// For the entry point of all secondary harts see [`_start_secondary`].
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
        "la     t0, __stack_start", // set the stack pointer to the bottom of the stack
        "li     t1, {stack_size}",  // load the stack size
        "mul    sp, a0, t1",        // multiply the stack size by the hart id to get the relative stack bottom offset
        "add    t0, t0, sp",        // add the relative stack bottom offset to the absolute stack region offset to get
                                    // the absolute stack bottom
        "add    sp, t0, t1",        // add one stack size again to get to the top of the stack. This is our final stack pointer.

        // fill stack with canary pattern
        // $sp is set to stack top above, $t0 as well
        "call   {fill_stack}",

        // Clear .bss.  The linker script ensures these are aligned to 16 bytes.
        "lla    a3, __bss_zero_start",
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

        stack_size = const crate::STACK_SIZE,
        start_rust = sym crate::main,
        fill_stack = sym fill_stack
    }
}

/// Entry point for all secondary harts, this is essentially the same as [`_start`] but it doesn't
/// attempt to zero out the BSS.
///
/// It will however transfer control to the common [`crate::main`] routine.
#[unsafe(naked)]
unsafe extern "C" fn _start_secondary() -> ! {
    naked_asm! {
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
        "la     t0, __stack_start", // set the stack pointer to the bottom of the stack
        "li     t1, {stack_size}",  // load the stack size
        "mul    sp, a0, t1",        // multiply the stack size by the hart id to get the relative stack bottom offset
        "add    t0, t0, sp",        // add the relative stack bottom offset to the absolute stack region offset to get
                                    // the absolute stack bottom
        "add    sp, t0, t1",        // add one stack size again to get to the top of the stack. This is our final stack pointer.

        // fill stack with canary pattern
        // $sp is set to stack top above, $t0 as well
        "call   {fill_stack}",

        // Call the rust entry point
        "call {start_rust}",

        // Loop forever.
        // `start_rust` should never return, but in case it does prevent the hart from executing
        // random code
        "2:",
        "   wfi",
        "   j 2b",

        stack_size = const crate::STACK_SIZE,
        start_rust = sym crate::main,
        fill_stack = sym fill_stack
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
        "li     t1, 0xACE0BACE",
        "1:",
        "   sw          t1, 0(t0)",     // write the canary as u64
        "   addi        t0, t0, 8",     // move to the next u64
        "   bltu        t0, sp, 1b",    // loop until we reach the top of the stack
        "ret"
    }
}

/// This will hand off control over this CPU to the kernel. This is the last function executed in
/// the loader and will never return.
pub unsafe fn handoff_to_kernel(hartid: usize, boot_ticks: u64, init: &GlobalInitResult) -> ! {
    let stack = init.stacks_allocation.region_for_cpu(hartid);
    let tls = init
        .maybe_tls_allocation
        .as_ref()
        .map(|tls| tls.region_for_hart(hartid))
        .unwrap_or_default();

    log::debug!("Hart {hartid} Jumping to kernel...");
    log::trace!(
        "Hart {hartid} entry: {}, arguments: a0={hartid} a1={:?} stack={stack:#x?} tls={tls:#x?}",
        init.kernel_entry,
        init.boot_info
    );

    // Synchronize all harts before jumping to the kernel.
    // Technically this isn't really necessary, but debugging output gets horribly mangled if we don't
    // and that's terrible for this critical transition
    init.barrier.wait();

    // Safety: inline assembly
    unsafe {
        riscv::sstatus::set_sum();

        asm! {
        "mv  sp, {stack_top}", // Set the kernel stack ptr
        "mv  tp, {tls_start}", // Set the kernel thread ptr

        // fill stack with canary pattern
        // $sp is set to stack top above, $t0 is set to stack bottom by the asm args below
        "call {fill_stack}",
        "mv ra, zero", // Reset return address

        "jalr zero, {kernel_entry}",

        // Loop forever.
        // The kernel should never return, but in case it does prevent the hart from executing
        // random code
        "1:",
        "   wfi",
        "   j 1b",
        in("a0") hartid,
        in("a1") init.boot_info.as_ptr(),
        in("a2") boot_ticks,
        in("t0") stack.start.get(),
        stack_top = in(reg) stack.end.get(),
        tls_start = in(reg) tls.start.get(),
        kernel_entry = in(reg) init.kernel_entry.get(),
        fill_stack = sym fill_stack,
        options(noreturn)
        }
    }
}

/// Start all secondary harts on the system as reported by [`MachineInfo`].
pub fn start_secondary_harts(boot_hart: usize, minfo: &MachineInfo) -> crate::Result<()> {
    let start = minfo.hart_mask.trailing_zeros() as usize;
    let end = (usize::BITS - minfo.hart_mask.leading_zeros()) as usize;
    log::trace!("{start}..{end}");

    for hartid in start..end {
        // Don't try to start ourselves
        if hartid == boot_hart {
            continue;
        }

        log::trace!("[{boot_hart}] starting hart {hartid}...");
        riscv::sbi::hsm::hart_start(
            hartid,
            _start_secondary as usize,
            minfo.fdt.as_ptr() as usize,
        )
        .map_err(Error::FailedToStartSecondaryHart)?;
    }

    Ok(())
}

pub fn init_address_space<F>(
    frame_allocator: &BootstrapAllocator<spin::RawMutex>,
    flush: &mut Flush,
) -> Result<BootstrapAddressSpace<kmem_core::arch::riscv64::Riscv64>, AllocError>
where
    F: FrameAllocator,
{
    const ROOT_ASID: u16 = 1;
    let arch = kmem_core::arch::riscv64::Riscv64::new(ROOT_ASID, satp::Mode::Sv39);

    BootstrapAddressSpace::new_bootstrap(arch, frame_allocator, flush)
}
