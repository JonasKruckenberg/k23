#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]

use crate::machine_info::MachineInfo;

mod arch;
mod logger;
mod machine_info;
mod panic;

fn main(hartid: usize, minfo: &'static MachineInfo) -> ! {
    log::info!(
        "Hello World from hart {hartid}, boot hart {}",
        minfo.boot_cpu
    );

    // Stage1: load kernel into ram
    // Stage2: map kernel

    todo!()
}
