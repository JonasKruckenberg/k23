use core::ops::Range;
use core::arch::asm;
use pmm::VirtualAddress;
use crate::arch::riscv64;
use crate::arch::riscv64::start;

#[no_mangle]
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
        fillstack = sym start::fillstack,
        options(noreturn)
    }
}