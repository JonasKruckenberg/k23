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
        } else {
            compile_error!("unsupported target architecture")
        }
    }
}
