#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]

extern crate alloc;

mod arch;
mod boot_info;
mod externs;
mod logger;
mod panic;
mod stack;

pub mod kconfig {
    // Configuration constants and statics defined by the build script
    include!(concat!(env!("OUT_DIR"), "/kconfig.rs"));
}

use crate::arch::BOOT_STACK;
use alloc::vec::Vec;

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;

fn main(_hartid: usize) -> ! {
    let stack_usage = BOOT_STACK.usage();
    log::debug!(
        "Stack usage: {} KiB of {} KiB total ({:.3}%). High Watermark: {} KiB.",
        (stack_usage.used) / KIB,
        (stack_usage.total) / KIB,
        (stack_usage.used as f64 / stack_usage.total as f64) * 100.0,
        (stack_usage.high_watermark) / KIB,
    );

    let _kernel = decompress_kernel();

    arch::halt()
}

fn decompress_kernel() -> Vec<u8> {
    let input = include_bytes!(env!("K23_KERNEL_ARTIFACT"));
    let output = lz4_flex::decompress_size_prepended(input).unwrap();
    log::debug!("decompressed kernel region {:?}", output.as_ptr_range());
    output
}
