#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]

mod boot_info;
mod logger;
mod panic;
mod stack;

pub mod kconfig {
    // Configuration constants and statics defined by the build script
    include!(concat!(env!("OUT_DIR"), "/kconfig.rs"));
}
