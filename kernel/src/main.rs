#![no_std]
#![no_main]
#![feature(naked_functions, asm_const, allocator_api, thread_local)]

extern crate alloc;

mod allocator;
mod arch;
mod boot_info;
mod kernel_mapper;
mod logger;
mod panic;
mod thread_local;

pub mod kconfig {
    // Configuration constants and statics defined by the build script
    include!(concat!(env!("OUT_DIR"), "/kconfig.rs"));
}

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xe57fad0f5f757433;

#[no_mangle]
pub unsafe extern "C" fn __stack_chk_fail() {
    panic!("Kernel stack is corrupted")
}
