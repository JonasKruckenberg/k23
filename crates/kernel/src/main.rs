#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, error_in_core)]

mod arch;
mod backtrace;
mod board_info;
mod error;
mod logger;
mod paging;
mod panic;
mod sync;

pub use error::Error;
pub type Result<T> = core::result::Result<T, Error>;

/// This is the main function of the kernel.
///
/// After performing arch & board specific initialization, all harts will end up in this function.
/// This function should set up hart-local state, and then ?. It should never return.
pub fn kmain(hartid: usize) -> ! {
    // arch-agnostic initialization
    // per-hart initialization

    log::info!("Hello world from hart {hartid}!");

    arch::trap::init().unwrap();

    todo!()
}
