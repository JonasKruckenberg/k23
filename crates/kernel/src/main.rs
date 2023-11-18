#![no_std]
#![no_main]
#![feature(naked_functions, asm_const)]

use core::arch::asm;

const STACK_SIZE_PAGES: usize = 25;
const PAGE_SIZE: usize = 4096;

#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    asm!(
        "la     sp, __stack_start", // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}", // load the stack size
        "mv     t1, a0",            // load the hart id
        "addi   t1, t1, 1", // add one to the hart id so that we add at least one stack size (stack grows from the top downwards)
        "mul    t0, t0, t1", // multiply the stack size by the hart id to get the offset
        "add    sp, sp, t0", // add the offset from sp to get the harts stack pointer

        "jal zero, {start_rust}", // jump into Rust

        stack_size = const STACK_SIZE_PAGES * PAGE_SIZE,
        start_rust = sym start,
        options(noreturn)
    )
}

extern "C" fn start(hartid: usize, opaque: usize) -> ! {
    extern "C" {
        static mut __bss_start: u64;
        static mut __bss_end: u64;
    }
    unsafe {
        let mut ptr = &mut __bss_start as *mut u64;
        let end = &mut __bss_end as *mut u64;
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }

    loop {
        unsafe {
            asm!("wfi");
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe {
            asm!("wfi");
        }
    }
}
