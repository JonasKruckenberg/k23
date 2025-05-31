cfg_if::cfg_if! {
    if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
        mod riscv;
        pub use riscv::*;
    } else if #[cfg(target_arch = "aarch64")] {
        mod aarch64;
        pub use aarch64::*;
    } else if #[cfg(all(target_arch = "x86_64", not(windows)))] {
        mod x86_64;
        pub use x86_64::*;
    } else if #[cfg(all(target_arch = "x86_64", windows))] {
        mod x86_64_windows;
        pub use x86_64_windows::*;
    } else {
        compile_error!("Unsupported target architecture");
    }
}
