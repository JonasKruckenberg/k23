use crate::logger::LoggerInner;
use crate::machine_info::MachineInfo;
use core::arch::asm;
use core::mem::MaybeUninit;
use core::ops::Range;
use core::ptr::addr_of_mut;
use core::{mem, slice};
use dtb_parser::{DevTree, Error, Node, Strings, Visitor};
use spin::Once;

const STACK_SIZE_PAGES: usize = 16;
const PAGE_SIZE: usize = 4096;

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

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

    stack_size = const STACK_SIZE_PAGES * PAGE_SIZE,
    // trap_frame_size = const mem::size_of::<TrapFrame>(),
    start_rust = sym start,
    options(noreturn)
    )
}

unsafe extern "C" fn start(hartid: usize, opaque: *const u8) -> ! {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
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

        crate::logger::init();

        // {
        //     let mut w = LoggerInner;
        //     let mut dbg = dtb_parser::debug::DebugVisitor::new(&mut w);
        //
        //     DevTree::from_raw(opaque).unwrap().visit(&mut dbg).unwrap()
        // }

        let minfo = MachineInfo::from_dtb(opaque);
        log::info!("{minfo:?}");

        // for hart in 0..8 {
        //     if hart != hartid {
        //         sbicall::hsm::start_hart(hart, _start as usize, 0).unwrap();
        //     }
        // }
    });

    crate::main(hartid)
}
