use crate::boot_info::BootInfo;
use crate::{kconfig, logger, KernelArgs};
use core::arch::asm;
use core::ptr::addr_of_mut;
use spin::Once;
use vmm::VirtualAddress;

// do global, arch-specific setup
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

        "jal zero, {start_rust}",   // jump into Rust
        stack_size = const kconfig::PAGE_SIZE * kconfig::STACK_SIZE_PAGES,

        start_rust = sym start,
        options(noreturn)
    )
}

// do local, arch-specific setup
#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start_hart() -> ! {
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

        "jal zero, {start_rust}",   // jump into Rust
        stack_size = const kconfig::PAGE_SIZE * kconfig::STACK_SIZE_PAGES,

        start_rust = sym crate::main,
        options(noreturn)
    )
}

static BOOT_INFO: Once<BootInfo> = Once::new();

fn start(hartid: usize, opaque: *const u8) -> ! {
    extern "C" {
        static mut __bss_start: u64;
        static mut __bss_end: u64;
    }

    unsafe {
        // Zero BSS section
        let mut ptr = addr_of_mut!(__bss_start);
        let end = addr_of_mut!(__bss_end);
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }

    logger::init();

    let boot_info = BOOT_INFO.call_once(|| BootInfo::from_dtb(opaque));

    log::debug!("{boot_info:?}");

    for hart in 0..boot_info.cpus {
        if hart != hartid {
            sbicall::hsm::start_hart(hart, _start_hart as usize, boot_info as *const _ as usize)
                .unwrap();
        }
    }

    crate::main(hartid, boot_info)
}

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

pub unsafe extern "C" fn kernel_entry(
    hartid: usize,
    stack_ptr: VirtualAddress,
    thread_ptr: VirtualAddress,
    func: VirtualAddress,
    args: &KernelArgs,
) -> ! {
    log::debug!("Hart {hartid} Jumping to kernel ({func:?})...");
    log::trace!("Hart {hartid} Kernel arguments: sp = {stack_ptr:?}, tp = {thread_ptr:?}, a0 = {hartid}, a1 = {args:p}");

    asm!(
        "mv sp, {stack_ptr}",
        "mv tp, {thread_ptr}",
        "mv ra, zero",
        "jalr zero, {func}",
        "1:",
        "   wfi",
        "   j 1b",
        in("a0") hartid,
        in("a1") args,
        stack_ptr = in(reg) stack_ptr.as_raw(),
        thread_ptr = in(reg) thread_ptr.as_raw(),
        func = in(reg) func.as_raw(),
        options(noreturn)
    )
}
