#![allow(unused)]

pub use pmm::arch::*;

cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
        pub use riscv::*;
    } else {
        compile_error!("Unsupported target architecture");
    }
}
