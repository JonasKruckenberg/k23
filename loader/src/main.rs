#![no_std]
#![no_main]
#![feature(naked_functions)]

mod panic;
mod arch;
mod machine_info;
mod error;
mod pmm;
mod page_alloc;

pub use error::Error;
pub type Result<T> = core::result::Result<T, Error>;

pub const ENABLE_KASLR: bool = false;

// What we need to do
// - setup stack ptr                    arch/cpu
// - fill stack with canary pattern     arch/cpu
// - disable interrupts                 arch/cpu

// - zero BSS                           global/global
// - initialize logger                  global/global

// - parse DTB                          arch/global
// - identity map self                  arch/global

// - map physical memory                global/global
// - map kernel elf                     global/global
//      - map load segments
//      - allocate & map TLS segment
//      - apply relocations
//      - process RELRO segments
// - map kernel stacks                  global/global

// - initialize TLS                     global/cpu
// - switch to kernel address space     global/cpu

fn main() {
    loop {}
}