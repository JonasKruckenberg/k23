#![allow(unused_imports)]

cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
        pub use pmm::arch::*;
    } else {
        compile_error!("Unsupported target architecture");
    }
}
