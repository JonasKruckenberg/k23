#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, pointer_is_aligned)]

mod arch;
mod logger;
mod machine_info;
mod panic;

// Configuration constants and statics defined by the build script
include!(concat!(env!("OUT_DIR"), "/kconfig.rs"));

fn kmain(hartid: usize) -> ! {
    log::info!("Hello World from hart {hartid}!");

    panic!()
}
