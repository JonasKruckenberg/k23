use crate::boot_info::{BootInfo, BOOT_INFO};
use crate::kconfig;
use crate::stack::Stack;
use core::arch::asm;
use core::ptr::addr_of_mut;
use uart_16550::SerialPort;

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

        // fill our stack area with a fixed pattern
        // so that we can identify unused stack memory in dumps & calculate stack usage
        "li          t1, {stack_fill}",
        "la          t0, __stack_start",
        "100:",
        "sw          t1, 0(t0)",
        "addi        t0, t0, 4",
        "bltu        t0, sp, 100b",

        // "addi sp, sp, -{trap_frame_size}",
        // "csrrw x0, sscratch, sp", // sscratch points to the trap frame

        "jal zero, {start_rust}", // jump into Rust

        stack_size = const (Stack::GUARD_PAGES + Stack::SIZE_PAGES) * kconfig::PAGE_SIZE,
        stack_fill = const Stack::FILL_PATTERN,
        // trap_frame_size = const mem::size_of::<TrapFrame>(),
        start_rust = sym start,
        options(noreturn)
    )
}

// 0xffffffff80029000
// 0xffffffff80041000

unsafe extern "C" fn start(hartid: usize, opaque: *const u8) -> ! {
    // use `call_once` to do all global one-time initialization
    let info = BOOT_INFO.call_once(|| {
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

        let info = BootInfo::from_dtb(opaque);

        crate::logger::init(&info);

        {
            let mut port = SerialPort::new(
                info.serial.reg.start.as_raw(),
                info.serial.clock_frequency,
                38400,
            );

            let mut v = dtb_parser::debug::DebugVisitor::new(&mut port);

            dtb_parser::DevTree::from_raw(opaque)
                .unwrap()
                .visit(&mut v)
                .unwrap();
        }

        // init_paging(&info);

        // for i in 0..info.cpus {
        //     if i != hartid {
        //         sbicall::hsm::start_hart(i, _start as usize, opaque as usize).unwrap();
        //     }
        // }

        info
    });

    crate::kmain(hartid, info);
}
