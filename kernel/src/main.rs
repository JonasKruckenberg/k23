#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]

use crate::boot_info::BootInfo;

mod arch;
mod boot_info;
mod logger;
mod panic;
mod stack_guard;

#[no_mangle]
fn kmain(hartid: usize, _boot_info: &'static BootInfo) -> ! {
    log::info!("Hello World from hart {hartid}");

    // Stage1: load kernel into ram
    // Stage2: map kernel

    todo!()
}
