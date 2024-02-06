#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, error_in_core, allocator_api)]

extern crate alloc;

mod allocator;
mod arch;
mod backtrace;
mod board_info;
mod error;
mod logger;
mod paging;
mod panic;
mod runtime;
mod sync;

pub use error::Error;

pub(crate) type Result<T> = core::result::Result<T, Error>;

fn kmain(hartid: usize) -> ! {
    log::info!("Hello world from hart {hartid}!");

    runtime::compile_wasm(include_bytes!("../full.wasm"));

    todo!()
}
