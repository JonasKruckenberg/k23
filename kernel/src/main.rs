#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]

use crate::boot_info::BootInfo;

mod arch;
mod boot_info;
mod logger;
mod panic;
mod stack;

pub mod kconfig {
    // Configuration constants and statics defined by the build script
    include!(concat!(env!("OUT_DIR"), "/kconfig.rs"));
}

#[no_mangle]
fn kmain(hartid: usize, _boot_info: &'static BootInfo) -> ! {
    log::info!("Hello World from hart {hartid}");

    todo!()
}
