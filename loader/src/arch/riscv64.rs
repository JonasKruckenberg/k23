use crate::kconfig;
use crate::machine_info::MachineInfo;
use core::arch::asm;
use core::ops::Range;
use core::ptr;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicPtr, Ordering};
use kmm::VirtualAddress;
use loader_api::BootInfo;
use sync::OnceLock;

/// The main entry point for the loader
///
/// This sets up the global and stack pointer, as well as filling the stack with a known debug pattern.
#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    asm!(
        ".option push",
        ".option norelax",
        "la		gp, __global_pointer$",
        ".option pop",

        "la     sp, __stack_start", // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}", // load the stack size
        "addi   t1, a0, 1", // add one to the hart id so that we add at least one stack size (stack grows from the top downwards)
        "mul    t1, t0, t1", // multiply the stack size by the hart id to get the offset
        "add    sp, sp, t1", // add the offset from sp to get the harts stack pointer

        "call {fillstack}",

        "jal zero, {start_rust}",   // jump into Rust

        stack_size = const kconfig::STACK_SIZE_PAGES * kconfig::PAGE_SIZE,

        fillstack = sym fillstack,
        start_rust = sym start,
        options(noreturn)
    )
}

fn start(hartid: usize, opaque: *const u8) -> ! {
    // Disable interrupts. The payload will re-enable interrupts
    // when it's ready to handle them
    riscv::interrupt::disable();

    if hartid == 0 {
        zero_bss();

        semihosting_logger::init(kconfig::LOG_LEVEL.to_level_filter());

        RAW_MACHINE_INFO.store(opaque as *mut u8, Ordering::Relaxed);
        MACHINE_INFO
            .get_or_try_init(|| unsafe { MachineInfo::from_dtb(opaque) })
            .expect("failed to parse machine info");

        log::info!("{MACHINE_INFO:?}");
    }

    crate::main(hartid);
}

fn zero_bss() {
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
}

/// # Safety
///
/// expects the bottom of `stack_size` in `t0` and the top of stack in `sp`
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

static MACHINE_INFO: OnceLock<MachineInfo> = OnceLock::new();
static RAW_MACHINE_INFO: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());

pub fn machine_info() -> &'static MachineInfo<'static> {
    MACHINE_INFO.get().expect("MachineInfo not initialized")
}

pub unsafe fn switch_to_payload(
    hartid: usize,
    entry: VirtualAddress,
    stack: Range<VirtualAddress>,
    thread_ptr: VirtualAddress,
    boot_info: &'static BootInfo,
) -> ! {
    let stack_ptr = stack.end;
    let stack_size = stack_ptr.sub_addr(stack.start);

    log::debug!("Hart {hartid} Jumping to kernel ({entry:?})...");
    log::trace!("Hart {hartid} Kernel arguments: sp = {stack_ptr:?}, tp = {thread_ptr:?}, a0 = {hartid}, a1 = {boot_info:p}");

    asm!(
        "mv  sp, {stack_ptr}", // Set the kernel stack ptr

        //  fill stack with canary pattern
        "call {fillstack}",

        "mv tp, {thread_ptr}",  // Set thread ptr
        "mv ra, zero", // Reset return address

        "jalr zero, {func}", // Jump to kernel

        // We should never ever reach this code, but if we do just spin indefinitely
        "1:",
        "   wfi",
        "   j 1b",
        in("a0") hartid,
        in("a1") boot_info,
        in("t0") stack_size,
        stack_ptr = in(reg) stack_ptr.as_raw(),
        thread_ptr = in(reg) thread_ptr.as_raw(),
        func = in(reg) entry.as_raw(),
        fillstack = sym fillstack,
        options(noreturn)
    )
}
