use crate::boot_info::BootInfo;
use crate::kconfig;
use core::arch::asm;
use core::ptr::addr_of_mut;
use vmm::VirtualAddress;

#[link_section = ".bss.uninit"]
pub static BOOT_STACK: [u8; kconfig::PAGE_SIZE * kconfig::STACK_SIZE_PAGES] =
    [0; kconfig::PAGE_SIZE * kconfig::STACK_SIZE_PAGES];

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

        "jal zero, {start_rust}",   // jump into Rust
        stack = sym BOOT_STACK,
        stack_size = const kconfig::PAGE_SIZE * kconfig::STACK_SIZE_PAGES,

        start_rust = sym start,
        options(noreturn)
    )
}

unsafe extern "C" fn start(hartid: usize, opaque: *const u8) -> ! {
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

    log::debug!("stack region {:?}", BOOT_STACK.as_ptr_range());

    let boot_info = BootInfo::from_dtb(opaque);

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
    stack_ptr: VirtualAddress,
    func: VirtualAddress,
    args: VirtualAddress,
) -> ! {
    log::debug!("jumping to kernel! stack_ptr: {stack_ptr:?}, func: {func:?}, args: {args:?}");

    asm!(
        "mv sp, {stack}",
        "mv ra, zero",
        "jalr zero, {func}",
        "1:",
        "   wfi",
        "   j 1b",
        in("a0") args.as_raw(),
        stack = in(reg) stack_ptr.as_raw(),
        func = in(reg) func.as_raw(),
        options(noreturn)
    )
}
