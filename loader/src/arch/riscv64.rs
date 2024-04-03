use crate::boot_info::BootInfo;
use crate::kconfig;
use crate::stack::Stack;
use core::arch::asm;
use core::ptr::addr_of_mut;
use vmm::PhysicalAddress;

#[link_section = ".bss.uninit"]
pub static BOOT_STACK: Stack = Stack::ZERO;

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
        "la     sp, {stack}",       // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}",  // load the stack size
        "add    sp, sp, t0",        // add the stack size to the stack pointer
        "mv     a2, sp",

        // fill our stack area with a fixed pattern
        // so that we can identify unused stack memory in dumps & calculate stack usage
        "li          t1, {stack_fill}",
        "la          t0, {stack}",
        "100:",
        "sw          t1, 0(t0)",
        "addi        t0, t0, 4",
        "bltu        t0, sp, 100b",

        "jal zero, {start_rust}",   // jump into Rust
        stack = sym BOOT_STACK,
        stack_size = const (Stack::GUARD_PAGES + Stack::SIZE_PAGES) * kconfig::PAGE_SIZE,
        stack_fill = const Stack::FILL_PATTERN,
        start_rust = sym start,
        options(noreturn)
    )
}

#[no_mangle]
unsafe extern "C" fn start(hartid: usize, opaque: *const u8, stack_base: PhysicalAddress) -> ! {
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

    let boot_stack_region = BOOT_STACK.region();
    debug_assert!(
        boot_stack_region.contains(&stack_base),
        "region {boot_stack_region:?} stack_base {stack_base:?}"
    );
    log::trace!("boot stack region {boot_stack_region:?} stack base {stack_base:?}");

    let boot_info = BootInfo::from_dtb(opaque);

    crate::main(hartid, boot_info)
}
