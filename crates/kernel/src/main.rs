#![no_std]
#![no_main]
#![feature(naked_functions)]

use core::arch::asm;

#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    asm!("1:", "wfi", "j 1b", options(noreturn))
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe {
            asm!("wfi");
        }
    }
}
