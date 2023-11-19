use crate::board_info::BoardInfo;
use crate::{logger, sbi, PAGE_SIZE, STACK_SIZE_PAGES};
use core::arch::asm;

/// Sets the harts stack pointer to the top of the stack.
///
/// Since all stacks are laid out sequentially in memory, starting at the `__stack_start` symbol,
/// we can calculate the stack pointer for each hart by adding the stack size multiplied by the
/// hart ID to the `__stack_start` symbol.
///
/// Therefore the hart ID essentially acts as an index into the stack area.
///
/// # Safety
///
/// The caller must ensure the hart ID is passed in `a0`.
#[naked]
unsafe extern "C" fn set_stack_pointer() {
    asm!(
        "la     sp, __stack_start", // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}", // load the stack size
        "addi   t1, a0, 1", // add one to the hart id so that we add at least one stack size (stack grows from the top downwards)
        "mul    t0, t0, t1", // multiply the stack size by the hart id to get the offset
        "add    sp, sp, t0", // add the offset from sp to get the harts stack pointer
        "ret",

        stack_size = const STACK_SIZE_PAGES * PAGE_SIZE,
        options(noreturn)
    )
}

/// This is the boot harts entry point into the kernel.
/// It is the first function that is called after OpenSBI has set up the environment.
///
/// Because we want to jump into Rust as soon as possible, we only set up the stack
/// pointer and move on.
#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    asm!(
        "call {set_stack_pointer}",

        "jal zero, {start_rust}", // jump into Rust

        set_stack_pointer = sym set_stack_pointer,
        start_rust = sym start,
        options(noreturn)
    )
}

/// This is the entry point for all other harts that aren't the boot hart.
/// This function is called after initializing the kernel by the boot hart through HSM `start_hart`.
///
/// As with the boot hart, we only set up the stack pointer and jump into Rust.
/// But since all global state has already been initialized by the boot hart, and hart-local
/// state will be set up in `kmain` there is no `start_hart` function, we directly move on to `kmain`.
#[no_mangle]
#[naked]
unsafe extern "C" fn _start_hart() -> ! {
    asm!(
        "call {set_stack_pointer}",

        "jal zero, {start_rust}", // jump into Rust

        set_stack_pointer = sym set_stack_pointer,
        start_rust = sym crate::kmain,
        options(noreturn)
    )
}

/// This is the init function of the kernel.
///
/// This function will take care of initializing all global state (not per-hart state)
/// such as parsing `BoardInfo`, initializing the logger, and setting up the kernel heap.
///
/// It will then start all other harts and jump into `kmain`.
extern "C" fn start(hartid: usize, opaque: *const u8) -> ! {
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

    let board_info = BoardInfo::from_raw(opaque).unwrap();

    logger::init(&board_info.serial, 38400);

    // TODO setup kernel heap

    for hart in 0..board_info.cpus {
        if hart != hartid {
            sbi::hsm::start_hart(hart, _start_hart as usize, 0).unwrap();
        }
    }

    crate::kmain(hartid)
}
