#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]
#![feature(isqrt)]

use crate::arch::{BOOT_STACK, PAGE_SIZE, STACK_SIZE_PAGES};
use core::arch::asm;
use core::ops::{Add, Range};

mod arch;
mod boot_info;
mod logger;
mod panic;

const STACK_FILL: u64 = 0xACE0BACE;

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;
pub const GIB: usize = 1024 * MIB;

fn main(hartid: usize) -> ! {
    let stack_usage = stack_usage();
    log::debug!(
        "Stack usage: {} KiB of {} KiB total ({:.3}%). High Watermark: {} KiB.",
        (stack_usage.used) / KIB,
        (stack_usage.total) / KIB,
        (stack_usage.used as f64 / stack_usage.total as f64) * 100.0,
        (stack_usage.used) / KIB,
    );

    let _use_stack = [0u8; 55 * KIB];

    // let _use_stack = [0u8; 1024 * 1024];

    arch::halt()
}

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xe57fad0f5f757433;

#[no_mangle]
pub unsafe extern "C" fn __stack_chk_fail() {
    panic!("Loader stack is corrupted")
}

fn stack_high_watermark(stack_region: Range<*const u8>) -> *const u64 {
    unsafe {
        let mut ptr = stack_region.start as *const u64;
        let stack_top = stack_region.end as *const u64;

        while ptr < stack_top && *ptr == STACK_FILL {
            ptr = ptr.offset(1);
        }

        ptr
    }
}

#[derive(Debug)]
struct StackUsage {
    used: usize,
    total: usize,
    high_watermark: usize,
}

fn stack_usage() -> StackUsage {
    let sp: usize;
    unsafe {
        asm!("mv {}, sp", out(reg) sp);
    }

    // let stack_bottom = BOOT_STACK.as_ptr() as usize;
    let stack_region = unsafe {
        BOOT_STACK.as_ptr().add(8 * PAGE_SIZE)..BOOT_STACK.as_ptr().add(BOOT_STACK.len())
    };

    let high_watermark = stack_high_watermark(stack_region.clone()) as usize;

    if sp < stack_region.start as usize {
        panic!("stack overflow");
    }

    // log::debug!("bottom: {stack_bottom:#x} sp: {sp:#x}");

    StackUsage {
        used: stack_region.end as usize - sp,
        total: BOOT_STACK.len() - 8 * PAGE_SIZE,
        high_watermark: stack_region.end as usize - high_watermark,
    }
}
