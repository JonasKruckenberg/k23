#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]
#![feature(isqrt)]

use crate::arch::{BOOT_STACK, PAGE_SIZE};

mod arch;
mod boot_info;
mod logger;
mod panic;
mod stack;

const STACK_FILL: u64 = 0xACE0BACE;

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;
pub const GIB: usize = 1024 * MIB;

fn main(hartid: usize) -> ! {
    let stack_usage = BOOT_STACK.usage();
    log::debug!(
        "Stack usage: {} KiB of {} KiB total ({:.3}%). High Watermark: {} KiB.",
        (stack_usage.used) / KIB,
        (stack_usage.total) / KIB,
        (stack_usage.used as f64 / stack_usage.total as f64) * 100.0,
        (stack_usage.used) / KIB,
    );

    let _use_stack = [0u8; 50 * KIB];

    arch::halt()
}
