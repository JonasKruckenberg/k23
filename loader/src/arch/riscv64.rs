use crate::boot_info::BootInfo;
use crate::{kconfig, logger};
use core::arch::asm;
use core::mem;
use core::ops::Range;
use core::ptr::addr_of_mut;
use sync::Once;
use vmm::VirtualAddress;

/// # Safety
///
/// expects the bottom of stack_size in `t0` and the top of stack in `sp`
#[naked]
unsafe extern "C" fn fillstack() {
    // fill our stack area with a fixed pattern
    // so that we can identify unused stack memory in dumps & calculate stack usage
    asm!(
        "li          t1, 0xACE0BACE",
        "sub         t0, sp, t0", // subtract stack_size from sp to get the bottom of stack
        "100:",
        "sw          t1, 0(t0)",
        "addi        t0, t0, 8",
        "bltu        t0, sp, 100b",
        "ret",
        options(noreturn)
    )
}

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
        "mul    t1, t0, t1", // multiply the stack size by the hart id to get the offset
        "add    sp, sp, t1", // add the offset from sp to get the harts stack pointer

        "call {fillstack}",

        "jal zero, {start_rust}",   // jump into Rust
        stack_size = const kconfig::PAGE_SIZE * kconfig::STACK_SIZE_PAGES,

        fillstack = sym fillstack,
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
        "mul    t1, t0, t1", // multiply the stack size by the hart id to get the offset
        "add    sp, sp, t1", // add the offset from sp to get the harts stack pointer

        "call {fillstack}",

        "jal zero, {start_rust}",   // jump into Rust
        stack_size = const kconfig::PAGE_SIZE * kconfig::STACK_SIZE_PAGES,

        fillstack = sym fillstack,
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

    let boot_info = BOOT_INFO.get_or_init(|| BootInfo::from_dtb(opaque));

    log::debug!("{boot_info:?}");

    for hart in 0..boot_info.cpus {
        if hart != hartid {
            riscv::sbi::hsm::start_hart(hart, _start_hart as usize, boot_info as *const _ as usize)
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

#[repr(C, align(16))]
#[derive(Debug)]
pub struct KernelArgs {
    boot_hart: u32,
    fdt_virt: VirtualAddress,
    stack_start: VirtualAddress,
    stack_end: VirtualAddress,
    page_alloc_offset: VirtualAddress,
    frame_alloc_offset: usize,
    loader_start: VirtualAddress,
    loader_end: VirtualAddress,
}

pub unsafe fn kernel_entry(
    hartid: usize,
    thread_ptr: VirtualAddress,
    func: VirtualAddress,
    boot_hart: u32,
    fdt_virt: VirtualAddress,
    stack: Range<VirtualAddress>,
    loader: Range<VirtualAddress>,
    page_alloc_offset: VirtualAddress,
    frame_alloc_offset: usize,
) -> ! {
    let stack_ptr = stack.end.sub(mem::size_of::<KernelArgs>());

    let kargs = &mut *(stack_ptr.as_raw() as *mut KernelArgs);
    kargs.boot_hart = boot_hart;
    kargs.fdt_virt = fdt_virt;
    kargs.stack_start = stack.start;
    kargs.stack_end = stack.end;
    kargs.page_alloc_offset = page_alloc_offset;
    kargs.frame_alloc_offset = frame_alloc_offset;
    kargs.loader_start = loader.start;
    kargs.loader_end = loader.end;

    log::debug!("Hart {hartid} Jumping to kernel ({func:?})...");
    log::trace!("Hart {hartid} Kernel arguments: sp = {stack_ptr:?}, tp = {thread_ptr:?}, a0 = {hartid}, a1 = {kargs:p}");

    let stack_size = stack_ptr.sub_addr(stack.start);

    asm!(
        "mv  sp, {stack_ptr}", // Set the kernel stack ptr

        //  fill stack with canary pattern
        "call {fillstack}",

        "mv tp, {thread_ptr}",  // Set thread ptr
        "mv ra, zero", // Reset return address
        "mv a1, sp", // Pass the KernelArgs ptr

        "jalr zero, {func}", // Jump to kernel

        // We should never ever reach this code, but if we do just spin indefinitely
        "1:",
        "   wfi",
        "   j 1b",
        in("a0") hartid,
        in("t0") stack_size,
        stack_ptr = in(reg) stack_ptr.as_raw(),
        thread_ptr = in(reg) thread_ptr.as_raw(),
        func = in(reg) func.as_raw(),
        fillstack = sym fillstack,
        options(noreturn)
    )
}
