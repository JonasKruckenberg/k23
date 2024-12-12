cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
    } else if #[cfg(target_arch = "aarch64")] {
        mod aarch64;
        pub use aarch64::*;
    }
}

pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;

/// The number of page table entries in one table
pub const PAGE_TABLE_ENTRIES: usize = 1 << PAGE_ENTRY_SHIFT;
