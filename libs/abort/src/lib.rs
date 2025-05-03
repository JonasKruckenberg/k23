#![cfg_attr(not(test), no_std)]

#[unsafe(no_mangle)]
#[inline(never)]
pub fn abort() -> ! {
    cfg_if::cfg_if! {
        if #[cfg(not(target_os = "none"))] {
            extern crate std;
            std::process::abort();
        } else if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            compile_error!("unsupported target architecture")
        }
    }
}
