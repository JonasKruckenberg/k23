// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::arch::naked_asm;

use mem_core::arch::riscv64::Riscv64Sv39;
use mem_core::{HardwareAddressSpace, VirtualAddress};
use riscv::{interrupt, satp, sbi};

use crate::KernelAspaceLayout;
use crate::kernel::RelocatedKernel;

/// The kernel address space while the loader is still constructing it — i.e.
/// before the MMU is switched on and control is handed to the kernel.
pub type KernelAspace = HardwareAddressSpace<Riscv64Sv39>;

pub fn disable_interrupts() {
    // disarm any timers that might have been set by firmware
    let _ = sbi::time::set_timer(u64::MAX);

    // disable interrupts
    interrupt::disable();
}

pub fn get_ticks() -> u64 {
    riscv::register::time::read64()
}

/// Transfers control to the kernel ELF
///
/// # Safety
///
/// the caller needs to ensure a number of prerequisites:
///
/// 1. the kernel must loaded and mapped into the kernel address space, BSS segments zeroed, and relocations applied
/// 2. boot hart stack and TLS block must be mapped into the kernel address space and initialized
/// 3. physical memory must be mapped into the kernel address space
/// 4. UART device (if any) must be mapped into the kernel address space
/// 5. interrupts must be disabled
/// 6. the handoff trampoline's own code pages must be identity-mapped (phys == virt)
///    in `aspace`, so the instruction fetch immediately after `csrw satp` (which turns
///    the MMU on) resolves at the trampoline PC rather than faulting.
pub unsafe fn handoff(
    aspace_layout: KernelAspaceLayout,
    kernel: &RelocatedKernel,
    aspace: KernelAspace,
) -> ! {
    let (arch, root_page_table) = aspace.into_raw_parts();

    let ppn = root_page_table.get() >> 12_i32;
    let satp = (satp::Mode::Sv39 as usize) << 60u32 | ((arch.asid() as usize) << 44u32) | ppn;

    // Safety: ensured by caller
    unsafe {
        handoff_trampoline(
            aspace_layout.boot_info.start,
            aspace_layout.boot_hart_stack.end,
            aspace_layout.boot_hart_tls.start,
            kernel.entry(),
            satp,
        )
    }
}

/// Transfers control to the kernel ELF
///
/// # Safety
///
/// the caller needs to ensure a number of prerequisites:
///
/// 1. the kernel must loaded and mapped into the kernel address space, BSS segments zeroed, and relocations applied
/// 2. boot hart stack and TLS block must be mapped into the kernel address space and initialized
/// 3. physical memory must be mapped into the kernel address space
/// 4. UART device (if any) must be mapped into the kernel address space
/// 5. interrupts must be disabled
/// 6. the trampoline's own code pages must be identity-mapped (phys == virt) in the
///    table loaded into `satp_value`, so the instruction fetch immediately after
///    `csrw satp` (which turns the MMU on) resolves at the trampoline PC rather than
///    faulting. `mapping::map_handoff_trampoline` establishes this mapping.
#[unsafe(link_section = ".text.handoff_trampoline")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn handoff_trampoline(
    boot_info: VirtualAddress,
    stack_top: VirtualAddress,
    tls: VirtualAddress,
    entry: VirtualAddress,
    satp_value: usize,
) -> ! {
    naked_asm! {
        // set SATP
        "csrw satp, a4",
        "sfence.vma zero, zero",

        "mv  sp, a1", // Set the kernel stack ptr
        "mv  tp, a2", // Set the kernel thread ptr
        "mv ra, zero", // Reset return address

        "jalr zero, a3",

        "1:",
        "   wfi",
        "   j 1b"
    }
}
