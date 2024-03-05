#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, maybe_uninit_uninit_array_transpose)]

mod arch;
mod logger;
mod machine_info;
mod panic;
mod stack_vec;

fn main(hartid: usize) -> ! {
    log::info!("Hello World from hart {hartid}");

    // Stage1: load kernel into ram
    // Stage2: map kernel

    arch::halt();
}
