// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod asid_allocator;
mod block_on;
pub mod device;
mod mem;
mod setjmp_longjmp;
pub mod state;
mod trap_handler;

use core::arch::asm;

pub use asid_allocator::AsidAllocator;
pub use block_on::block_on;
pub use mem::{
    AddressSpace, CANONICAL_ADDRESS_MASK, DEFAULT_ASID, KERNEL_ASPACE_RANGE, PAGE_SHIFT, PAGE_SIZE,
    USER_ASPACE_RANGE, invalidate_range, is_kernel_address,
};
pub use setjmp_longjmp::{JmpBuf, JmpBufStruct, call_with_setjmp, longjmp};
pub use x86::*;

use crate::arch::device::cpu::Cpu;
use crate::device_tree::DeviceTree;
use crate::mem::VirtualAddress;

pub const STACK_ALIGNMENT: usize = 16;

/// Global x86_64 specific initialization.
#[cold]
pub fn init() -> state::Global {
    mem::init();
    asid_allocator::init();

    state::Global {}
}

/// Early per-cpu and x86_64 specific initialization.
///
/// This function will be called before global initialization is done, notably this function
/// cannot call logging functions, cannot allocate memory, cannot access cpu-local state and should
/// not panic as the panic handler is not initialized yet.
#[cold]
pub fn per_cpu_init_early() {
    unsafe {
        // FS segment base register is mysteriously gets cleared when jumping from
        // the loader to the kernel entry.
        // this assembly below sets the FS_BASE MSR to the correct value
        // TODO: figure out why this happens
        // The TLS region starts at 0xffffffc080001000 as shown in loader output
        core::arch::asm!(
            "mov rcx, 0xc0000100",  // FS_BASE MSR
            "mov rax, 0xffffffc080001000",  // Low 32 bits of TLS base
            "mov rdx, 0xffffffc0",           // High 32 bits of TLS base
            "wrmsr",
            out("rcx") _,
            out("rax") _,
            out("rdx") _,
        );

        // Initialize x87 FPU
        core::arch::asm!("fninit");

        // Initialize SSE/SSE2 state
        // Set CR4.OSFXSR to enable SSE instructions
        core::arch::asm!(
            "mov rax, cr4",
            "or rax, 0x200",  // CR4.OSFXSR (bit 9)
            "mov cr4, rax",
            out("rax") _,
        );

        // Set CR4.OSXMMEXCPT to enable unmasked SSE exceptions
        core::arch::asm!(
            "mov rax, cr4",
            "or rax, 0x400",  // CR4.OSXMMEXCPT (bit 10)
            "mov cr4, rax",
            out("rax") _,
        );

        // Enable RDTSC instruction for user mode by clearing CR4.TSD
        // (TSD = Time Stamp Disable for user mode)
        core::arch::asm!(
            "mov rax, cr4",
            "and rax, {tsd_mask}",  // Clear CR4.TSD (bit 2)
            "mov cr4, rax",
            tsd_mask = const !0x4u64,
            out("rax") _,
        );
    }
}

/// Late per-cpu and x86_64 specific initialization.
///
/// This function will be called after all global initialization is done.
#[cold]
pub fn per_cpu_init_late(devtree: &DeviceTree, cpuid: usize) -> crate::Result<state::CpuLocal> {
    // Initialize the trap handler
    trap_handler::init();

    Ok(state::CpuLocal {
        cpu: Cpu::new(devtree, cpuid)?,
    })
}

/// Set the thread pointer on the calling cpu to the given address.
pub fn set_thread_ptr(addr: VirtualAddress) {
    panic!("x86_64: set_thread_ptr not implemented");
    // TODO: Implement thread pointer setting for x86_64
    // On x86_64, this might use the FS or GS segment base
}

#[inline]
/// Returns the current stack pointer.
pub fn get_stack_pointer() -> usize {
    let stack_pointer: usize;
    unsafe {
        asm!(
            "mov {}, rsp",
            out(reg) stack_pointer,
            options(nostack,nomem),
        );
    }
    stack_pointer
}

/// Retrieves the next older program counter and stack pointer from the current frame pointer.
pub unsafe fn get_next_older_pc_from_fp(fp: VirtualAddress) -> VirtualAddress {
    // The calling convention always pushes the return pointer (aka the PC of
    // the next older frame) just before this frame.
    unsafe { *(fp.as_ptr() as *mut VirtualAddress).offset(1) }
}

/// The current frame pointer points to the next older frame pointer.
pub const NEXT_OLDER_FP_FROM_FP_OFFSET: usize = 0;

/// Asserts that the frame pointer is sufficiently aligned for the platform.
pub fn assert_fp_is_aligned(fp: VirtualAddress) {
    assert_eq!(fp.get() % 16, 0, "stack should always be aligned to 16");
}

pub fn mb() {
    unsafe {
        asm!("mfence");
    }
}

pub fn wmb() {
    unsafe {
        asm!("sfence");
    }
}

pub fn rmb() {
    unsafe {
        asm!("lfence");
    }
}

/// Terminates the current execution with the specified exit code.
pub fn exit(_code: i32) -> ! {
    // TODO: Implement proper x86_64 exit mechanism
    // For now, just halt the CPU
    loop {
        unsafe {
            asm!("hlt");
        }
    }
}
