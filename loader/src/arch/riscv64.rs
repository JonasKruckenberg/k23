use crate::machine_info::MachineInfo;
use crate::{VirtualAddress, LOG_LEVEL, STACK_SIZE_PAGES};
use core::arch::{asm, naked_asm};
use core::ops::Range;
use core::ptr::addr_of_mut;
use sync::Mutex;

/// The main entry point for the loader
///
/// This sets up the global and stack pointer, as well as filling the stack with a known debug pattern
/// and then - as fast as possible - jumps to Rust.
#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    naked_asm!(
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

        stack_size = const STACK_SIZE_PAGES * 4096, // TODO make dynamic

        fillstack = sym fillstack,
        start_rust = sym start,
    )
}

/// Architecture specific startup code
fn start(hartid: usize, opaque: *const u8) -> ! {
    static INIT: sync::OnceLock<(MachineInfo, Mutex<pmm::Riscv64Sv39>)> = sync::OnceLock::new();

    // Pick a hart (whichever arrives here first) to perform global
    // initialization. All other harts will spin-wait here until it is done.
    let (minfo, pmm_arch) = INIT
        .get_or_try_init(|| -> crate::Result<_> {
            // zero out the BSS section, under QEMU we already get zeroed memory
            // but on actual hardware this might not be the case
            zero_bss();

            semihosting_logger::init(LOG_LEVEL.to_level_filter());

            let minfo = unsafe { MachineInfo::from_dtb(opaque)? };
            log::info!("{minfo:?}");

            Ok((minfo, Mutex::new(pmm::Riscv64Sv39)))
        })
        .expect("failed arch global initialization");

    log::trace!("[HART {hartid}] hart is booting...");

    crate::main(hartid, pmm_arch, minfo)
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

/// Fill the stack with a canary pattern (0xACE0BACE) so that we can identify unused stack memory
/// in dumps & calculate stack usage. This is also really great (don't ask my why I know this) to identify
/// when we tried executing stack memory.
///
/// # Safety
///
/// expects the bottom of `stack_size` in `t0` and the top of stack in `sp`
#[naked]
unsafe extern "C" fn fillstack() {
    naked_asm!(
        "li          t1, 0xACE0BACE",
        "sub         t0, sp, t0", // subtract stack_size from sp to get the bottom of stack
        "100:",
        "sw          t1, 0(t0)",
        "addi        t0, t0, 8",
        "bltu        t0, sp, 100b",
        "ret",
    )
}

pub unsafe fn handoff_to_kernel(
    hartid: usize,
    entry: VirtualAddress,
    stack: Range<VirtualAddress>,
    thread_ptr: VirtualAddress,
    boot_info: VirtualAddress,
) -> ! {
    let stack_ptr = stack.end;
    let stack_size = stack_ptr.sub_addr(stack.start);

    log::debug!("Hart {hartid} Jumping to kernel ({entry:?})...");
    log::trace!("Hart {hartid} Kernel arguments: sp = {stack_ptr:?}, tp = {thread_ptr:?}, a0 = {hartid}, a1 = {boot_info:?}");

    asm! {
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
        in("a1") boot_info.as_raw(),
        in("t0") stack_size,
        stack_ptr = in(reg) stack_ptr.as_raw(),
        thread_ptr = in(reg) thread_ptr.as_raw(),
        func = in(reg) entry.as_raw(),
        fillstack = sym fillstack,
        options(noreturn)
    }
}
