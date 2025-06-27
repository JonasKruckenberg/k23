// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(not(test), no_std)]

/// Terminates the current execution in an abnormal fashion. This function will never return.
///
/// The function will terminate the execution, either by some platform specific means. When `std`
/// is available, this will take the form of `std::process::abort`. On `no_std` targets this will
/// attempt to use [semihosting] to terminate the execution, and as a fallback put the CPU into an
/// idle loop forever.
///
/// [semihosting]: <https://developer.arm.com/documentation/dui0203/j/semihosting/about-semihosting/what-is-semihosting->
///
/// # Breakpoint support
///
/// The symbol `abort` will never be mangled so you can safely put a breakpoint on it
/// as a means to catch the kernel just before it exits abnormally.
#[unsafe(no_mangle)]
#[inline(never)]
pub fn abort() -> ! {
    cfg_if::cfg_if! {
        if #[cfg(not(target_os = "none"))] {
            extern crate std;
            std::process::abort();
        } else if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::exit(1);
        } else if #[cfg(target_arch = "x86_64")] {
            // For x86_64, we'll disable interrupts and halt forever
            unsafe {
                core::arch::asm!(
                    "cli",      // Clear interrupt flag
                    "2:",
                    "hlt",      // Halt the CPU
                    "jmp 2b",   // Loop just in case
                    options(noreturn)
                );
            }
        } else {
            compile_error!("unsupported target architecture")
        }
    }
}
