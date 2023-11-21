#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, error_in_core, allocator_api)]
#![feature(c_unwind)]

extern crate alloc;

mod board_info;
mod error;
mod kmem;
mod logger;
mod sbi;
mod start;
mod trap;
mod unwind;

use core::arch::asm;
use error::Error;

pub type Result<T> = core::result::Result<T, Error>;

const STACK_SIZE_PAGES: usize = 25;
const PAGE_SIZE: usize = 4096;

/// This is the main function of the kernel.
///
/// After performing arch & board specific initialization, all harts will end up in this function.
/// This function should set up hart-local state, and then ?. It should never return.
fn kmain(hartid: usize) -> ! {
    log::info!("Hello world from hart {hartid}!");

    trap::init();

    // sbi::time::set_timer(2_000_000).unwrap();

    panic!();

    loop {
        unsafe {
            asm!("wfi");
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let ctx = unwind::Context::capture();

    log::error!("KERNEL PANIC {}", info);
    log::debug!("ctx {:?}", ctx);

    let b = unwind::Backtrace::new();
    for frame in b {
        log::debug!("{:#19x}", frame.pc);
    }

    loop {
        unsafe {
            asm!("wfi");
        }
    }
}
