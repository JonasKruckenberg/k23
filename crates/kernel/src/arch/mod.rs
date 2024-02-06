cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
    } else {
        compile_error!("unsupported target architecture");
    }
}
