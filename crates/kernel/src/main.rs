#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, error_in_core, allocator_api, step_trait)]

extern crate alloc;

mod allocator;
mod arch;
mod backtrace;
mod board_info;
mod error;
mod logger;
mod panic;
mod runtime;
mod sync;
// mod vmm;

pub use error::Error;
pub type Result<T> = core::result::Result<T, Error>;

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;
pub const GIB: usize = 1024 * MIB;

fn kmain(hartid: usize) -> ! {
    log::info!("Hello, world!");

    if let Err(err) = runtime::compile_wasm(include_bytes!("../arith.wasm")) {
        panic!("failed to compile test wasm module {err}");
    }

    loop {
        arch::halt();
    }
}
