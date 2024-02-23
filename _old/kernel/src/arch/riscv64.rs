use crate::machine_info::MachineInfo;
use crate::{logger, KCONFIG, MEMORY_MODE};
use core::arch::asm;
use core::ptr::addr_of_mut;
use spin::Once;
use vmm::Mode;

#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    asm!(
        ".option push",
        ".option norelax",
        "    la		gp, __global_pointer$",
        ".option pop",

        "la     sp, __stack_start", // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}", // load the stack size
        "addi   t1, a0, 1", // add one to the hart id so that we add at least one stack size (stack grows from the top downwards)
        "mul    t0, t0, t1", // multiply the stack size by the hart id to get the offset
        "add    sp, sp, t0", // add the offset from sp to get the harts stack pointer

        // "addi sp, sp, -{trap_frame_size}",
        // "csrrw x0, sscratch, sp", // sscratch points to the trap frame

        "jal zero, {start_rust}", // jump into Rust

        stack_size = const KCONFIG.stack_size_pages * MEMORY_MODE::PAGE_SIZE,
        // trap_frame_size = const mem::size_of::<TrapFrame>(),
        start_rust = sym start,
        options(noreturn)
    )
}

static BOOT_ARGS: Once<()> = Once::new();

unsafe extern "C" fn start(hartid: usize, opaque: *const u8) -> ! {
    let boot_args = if hartid == 0 {
        extern "C" {
            static mut __bss_start: u64;
            static mut __bss_end: u64;
        }

        // Zero BSS section
        let mut ptr = addr_of_mut!(__bss_start);
        let end = addr_of_mut!(__bss_end);
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }

        let machine_info = MachineInfo::from_dtb(opaque);

        logger::init(&machine_info);

        BOOT_ARGS.call_once(|| {})
    } else {
        BOOT_ARGS.wait()
    };

    crate::kmain(hartid)
}
