#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, error_in_core, allocator_api)]
#![feature(c_unwind)]

extern crate alloc;

mod backtrace;
mod board_info;
mod error;
mod kmem;
mod logger;
mod start;
mod trap;

use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};
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

    loop {
        unsafe {
            asm!("wfi");
        }
    }
}

static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("KERNEL PANIC {}", info);

    // if we panic in the backtrace, prevent us from spinning into an infinite panic loop
    if !PANICKING.swap(true, Ordering::AcqRel) {
        log::error!("un-symbolized stack trace:");
        let mut count = 0;
        backtrace::trace(|frame| {
            count += 1;
            log::debug!("{:<2}- {:#x?}", count, frame.symbol_address());
        });
    }

    loop {
        unsafe {
            asm!("wfi");
        }
    }
}
