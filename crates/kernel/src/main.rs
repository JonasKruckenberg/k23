#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, error_in_core)]

mod board_info;
mod error;
mod logger;
mod sbi;
mod start;

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

    loop {
        unsafe {
            asm!("wfi");
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("KERNEL PANIC {}", info);

    loop {
        unsafe {
            asm!("wfi");
        }
    }
}
