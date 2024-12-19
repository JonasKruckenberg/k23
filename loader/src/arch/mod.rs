#![allow(unused_imports)]

cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
        pub use mmu::arch::*;
    } else {
        compile_error!("Unsupported target architecture");
    }
}

pub fn abort() -> ! {
    cfg_if::cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            loop {}
        }
    }
}
