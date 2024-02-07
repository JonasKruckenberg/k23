#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, error_in_core, allocator_api)]

mod allocator;
mod arch;
mod backtrace;
mod board_info;
mod error;
mod logger;
mod panic;
mod sync;

pub use error::Error;
pub type Result<T> = core::result::Result<T, Error>;

fn kmain(hartid: usize) -> ! {
    log::info!("Hello, world!");

    loop {
        arch::halt();
    }
}
