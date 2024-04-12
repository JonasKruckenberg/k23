#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]

mod arch;
mod panic;

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xe57fad0f5f757433;

#[no_mangle]
pub unsafe extern "C" fn __stack_chk_fail() {
    panic!("Kernel stack is corrupted")
}
