// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::utils::{EncodedValue, allocate_obj_on_stack, push};
use crate::{FiberStack, StackPointer};
use cfg_if::cfg_if;
use core::arch::{asm, naked_asm};

pub const STACK_ALIGNMENT: usize = 16;

#[inline]
pub unsafe fn init_stack<T>(
    stack: &dyn FiberStack,
    func: unsafe extern "C-unwind" fn(arg: EncodedValue, sp: &mut StackPointer, obj: *mut T) -> !,
    obj: T,
) -> (StackPointer, StackPointer) {
    // Safety: ensured by caller
    unsafe {
        let mut sp = stack.top().get();

        // Place the address of the initial function to execute at the top of the
        // stack. This is read by stack_init_trampoline() and jumped to.
        push(&mut sp, Some(func as usize));

        // Placeholder for the stack pointer value of the parent context. This is
        // filled in every time switch_and_link() is called.
        push(&mut sp, None);

        // Allocate space on the stack for the initial object, rounding to
        // STACK_ALIGNMENT.
        allocate_obj_on_stack(&mut sp, 16, obj);
        let init_obj = sp;

        // Set up an address at the top of the stack which is called by
        // switch_and_link() during the initial context switch.
        push(&mut sp, Some(stack_init_trampoline as usize));

        (
            StackPointer::new_unchecked(sp),
            StackPointer::new_unchecked(init_obj),
        )
    }
}

#[naked]
pub unsafe extern "C" fn stack_init_trampoline() {
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            ".balign 16",
            ".cfi_startproc",
            // This gets called by switch_and_link() the first time a fiber is
            // resumed, due to the initial state set up by init_stack().
            //
            // At this point our register state contains the following:
            // - RSP points to the top of the parent stack.
            // - RBP holds its value from the parent context.
            // - RDX points to the top of our stack.
            // - RSI points to the base of our stack.
            // - RDI contains the argument passed from switch_and_link.
            //
            // Save the RBP of the parent context to the parent stack. When combined
            // with the return address this forms a valid frame record (RBP & RIP) in
            // the frame pointer chain.
            "push rbp",
            // Fill in the parent link near the base of the stack. This is updated
            // every time we switch into a fiber and allows the fiber to
            // return to our context through the Suspend and when it unwinds.
            "mov [rsi - 16], rsp",
            // On entry RSI will be pointing to the stack base (see switch_and_link). We
            // need to adjust this to point to the parent link instead for the second
            // parameter of the entry function.
            "sub rsi, 16",
            // Switch to the fiber stack, skipping the address of
            // stack_init_trampoline() at the top of the stack.
            "lea rsp, [rdx + 8]",
            // Set up the frame pointer to point at the parent link. This is needed for
            // the unwinding code below.
            "mov rbp, rsi",
            // Tell the unwinder where to find the Canonical Frame Address (CFA) of the
            // parent context.
            //
            // The CFA is normally defined as the stack pointer value in the caller just
            // before executing the call instruction. In our case, this is the stack
            // pointer value that should be restored upon exiting the inline assembly
            // block inside switch_and_link().
            //
            // Once the unwinder reaches this function, it will have a virtual RBP value
            // pointing right at the parent link (see the diagram at the top of this
            // file). We need to use a custom DWARF expression to read this value off
            // the stack, and then add 24 bytes to skip over the 3 saved values on the
            // stack.
            ".cfi_escape 0x0f,  /* DW_CFA_def_cfa_expression */\
            5,                  /* the byte length of this expression */\
            0x76, 0x00,         /* DW_OP_breg6 (rbp + 0) */\
            0x06,               /* DW_OP_deref */\
            0x23, 0x18          /*DW_OP_plus_uconst 24*/",

            // Now we can tell the unwinder how to restore the 3 registers that were
            // pushed on the parent stack. These are described as offsets from the CFA
            // that we just calculated.
            ".cfi_offset rbx, -8",
            ".cfi_offset rip, -16",
            ".cfi_offset rbp, -24",
            // Set up the 3rd argument to the initial function to point to the object
            // that init_stack() set up on the stack.
            "mov rdx, rsp",
            // Rather than call the initial function with a CALL instruction, we
            // manually set up a return address and use JMP instead. This avoids a
            // misalignment of the CPU's return address predictor when a RET instruction
            // is later executed by a switch_yield() or switch_and_reset() in the
            // initial function. This is the reason why those functions are marked as
            // #[inline(always)].
            "lea rcx, [rip + 2f]",
            "push rcx",
            // init_stack() placed the address of the initial function just above the
            // parent link on the stack.
            "jmp [rsi + 8]",
            // We don't need to do anything afterwards since the initial function will
            // never return. This is guaranteed by the ! return type.
            //
            // Export the return target of the initial trampoline. This is used when
            // setting up a trap handler.
            "2:",
            // "int3", This int3 is necessary because of our use of .cfi_signal_frame earlier.
            ".cfi_endproc",
        }
    }
}

/// Transfer control to a fiber along with an argument.
///
/// This function will also store a pointer back to our stack therefore *linking* the two stacks.
/// This is required for correctly unwinding through the linked list of stacks.
#[inline]
pub unsafe fn switch_and_link(
    arg0: EncodedValue,
    sp: StackPointer,
    top_of_stack: StackPointer,
) -> (EncodedValue, Option<StackPointer>) {
    let (ret_val, ret_sp);

    // Safety: inline assembly
    unsafe {
        asm! {
            // Save RBX. Ideally this would be done by specifying them as a clobber
            // but that is not possible since RBX is an LLVM reserved register.
            //
            // RBP is also reserved but it is pushed onto the stack later after the
            // call so that a valid frame pointer record is created.
            "push rbx",

            // DW_CFA_GNU_args_size 0
            //
            // Indicate to the unwinder that this "call" does not take any arguments
            // and no stack space needs to be popped before executing a landing pad.
            // This is mainly here to undo the effect of any previous
            // DW_CFA_GNU_args_size that may have been set in the current function.
            ".cfi_escape 0x2e, 0x00",

            // Push a return address onto our stack and then jump to the return
            // address at the top of the fiber stack.
            //
            // From here on execution continues in stack_init_trampoline or the 2:
            // label in switch_yield.
            "call [rdx]",

            // Upon returning, our register state contains the following:
            // - RSP: Our stack, with the return address and RBP popped.
            // - RSI: The top of the fiber stack, or 0 if coming from
            //        switch_and_reset.
            // - RDI: The argument passed from the fiber.

            // The unwind state at this point is a bit tricky: the CFI instructions
            // in stack_init_trampoline will have already restored RBX even
            // though the program counter looks like it is pointing before the POP
            // instruction. However this doesn't cause any issues in practice.

            // Restore RBX.
            "pop rbx",

            // The RDI register is specifically chosen to hold the argument since
            // the ABI uses it for the first argument of a function call.
            //
            // This register is not modified in the assembly code, it is passed
            // straight through to the new context.
            inlateout("rdi") arg0 => ret_val,
            // The returned stack pointer can be in any register, RSI is arbitrarily
            // chosen here. This must match the register used in switch_yield() and
            // switch_and_reset().
            lateout("rsi") ret_sp,
            // Pass the top of stack in RSI so that on the first switch it is passed
            // as the second argument of the initial function. In
            // stack_init_trampoline this is adjusted to point to the parent
            // link directly.
            in("rsi") top_of_stack.get() as u64,
            // The target stack pointer can be in any register, RDX is arbitrarily
            // chosen here. This needs to match with the register expected by
            // switch_yield().
            in("rdx") sp.get() as u64,
            // Mark all registers as clobbered. Most of the work is done by
            // clobber_abi, we just add the remaining callee-saved registers here.
            // RBX and RBP are LLVM reserved registers and have to be manually
            // saved and restored in the assembly code.
            //
            // Doing this here is more efficient than manually saving all the
            // callee-saved registers: the compiler can avoid repeated saves and
            // restores when multiple context switches are called from the same
            // function.
            lateout("r12") _, lateout("r13") _, lateout("r14") _, lateout("r15") _,
            clobber_abi("sysv64"),
            options(may_unwind)
        }
    }

    (ret_val, StackPointer::new(ret_sp))
}

#[inline(always)]
pub unsafe fn switch_yield(arg: EncodedValue, parent_link: *mut StackPointer) -> EncodedValue {
    let ret_val;

    // Safety: inline assembly
    unsafe {
        asm! {
            // Save RBP and RBX. Ideally this would be done by specifying them as
            // clobbers but that is not possible since they are LLVM reserved
            // registers.
            "push rbp",
            "push rbx",

            // Push a return address on the stack. This is the address that will be
            // called by switch_and_link() the next time this context is resumed.
            "lea rax, [rip + 2f]",
            "push rax",

            // Save our stack pointer to RSI, which is then returned out of
            // switch_and_link().
            "mov rsi, rsp",

            // Load the parent context's stack pointer.
            "mov rsp, [rdx]",

            // Restore the parent's RBP register which is at the top of the stack.
            "pop rbp",

            // DW_CFA_GNU_args_size 0
            //
            // Indicate to the unwinder that this "call" does not take any arguments
            // and no stack space needs to be popped before executing a landing pad.
            // This is mainly here to undo the effect of any previous
            // DW_CFA_GNU_args_size that may have been set in the current function.
            //
            // This is needed here even though we don't call anything because
            // switch_and_throw may inject a call which returns to this point.
            ".cfi_escape 0x2e, 0x00",

            // Return into the parent context. This returns control to
            // switch_and_link() after the call instruction.
            "ret",

            // This gets called by switch_and_link(). At this point our register
            // state contains the following:
            // - RSP points to the top of the parent stack.
            // - RBP holds its value from the parent context.
            // - RDX points to the top of our stack, including the return address.
            // - RSI points to the base of our stack.
            // - RDI contains the argument passed from switch_and_link.
            "2:",

            // Save the RBP of the parent context to the parent stack. When combined
            // with the return address this forms a valid frame record (RBP & RIP)
            // in the frame pointer chain.
            "push rbp",

            // Update the parent link near the base of the stack. This is updated
            // every time we switch into a fiber and allows the fiber to
            // return to our context through the Yielder and when it unwinds.
            "mov [rsi - 16], rsp",

            // Switch back to our stack, skipping the return address.
            "lea rsp, [rdx + 8]",

            // Restore RBP and RBX.
            "pop rbx",
            "pop rbp",

            // RDI is used by switch_and_link to pass the argument in/out.
            inlateout("rdi") arg => ret_val,
            // The parent link can be in any register, RDX is arbitrarily chosen
            // here.
            in("rdx") parent_link as u64,
            // See switch_and_link() for an explanation of the clobbers.
            lateout("r12") _, lateout("r13") _, lateout("r14") _, lateout("r15") _,
            clobber_abi("sysv64"),
            options(may_unwind)
        }
    }

    ret_val
}

#[inline(always)]
pub unsafe fn switch_and_reset(arg: EncodedValue, parent_link: *mut StackPointer) -> ! {
    // Safety: inline assembly
    unsafe {
        asm! {
            // Load the parent context's stack pointer.
            "mov rsp, [{parent_link}]",

            // Restore the parent's RBP register which is at the top of the stack.
            "pop rbp",

            // Return into the parent context. The top of the parent stack contains
            // a return address generated by the CALL instruction in
            // switch_and_link().
            "ret",

            parent_link = in(reg) parent_link as u64,
            in("rdi") arg,
            // Hard-code the returned stack pointer value to 0 to indicate that this
            // fiber is done.
            in("rsi") 0,
            options(noreturn),
        }
    }
}

/// Variant of `switch_and_link` which runs a function on the coroutine stack
/// instead of resuming the coroutine. This function will throw an exception
/// which will unwind the coroutine stack to its root.
#[inline]
pub unsafe fn switch_and_throw(
    sp: StackPointer,
    top_of_stack: StackPointer,
) -> (EncodedValue, Option<StackPointer>) {
    extern "sysv64-unwind" fn throw() -> ! {
        extern crate alloc;
        use alloc::boxed::Box;

        // choose the right `panic_unwind` impl depending on whether the target supports `std`
        // or not
        cfg_if! {
            if #[cfg(target_os = "none")] {
                use panic_unwind::resume_unwind;
            } else {
                use std::panic::resume_unwind;
            }
        }

        resume_unwind(Box::new(()));
    }

    let (ret_val, ret_sp);

    // Safety: inline assembly
    unsafe {
        asm! {
            // Save RBX just like the first half of switch_and_link().
            "push rbx",

            // Push a return address to the stack.
            "lea rax, [rip + 2f]",
            "push rax",

            // Save RBP of the parent context.
            "push rbp",

            // Update the parent link near the base of the coroutine stack.
            "mov [rsi - 16], rsp",

            // Switch to the coroutine stack.
            "mov rsp, rdx",

            // Pop the return address of the target context.
            "pop rax",

            // Restore RBP and RBX from the target context.
            "pop rbx",
            "pop rbp",

            // DW_CFA_GNU_args_size 0
            //
            // Indicate to the unwinder that this "call" does not take any arguments
            // and no stack space needs to be popped before executing a landing pad.
            // This is mainly here to undo the effect of any previous
            // DW_CFA_GNU_args_size that may have been set in the current function.
            ".cfi_escape 0x2e, 0x00",

            // Simulate a call with an artificial return address so that the throw
            // function will unwind straight into the switch_and_yield() call with
            // the register state expected outside the asm! block.
            "push rax",
            "jmp {throw}",

            // Upon returning, our register state is just like a normal return into
            // switch_and_link().
            "2:",

            // Restore registers just like the second half of switch_and_link.
            "pop rbx",

            // Helper function to trigger stack unwinding.
            throw = sym throw,

            // Same output registers as switch_and_link().
            lateout("rdi") ret_val,
            lateout("rsi") ret_sp,

            // We pass the top of stack in rsi.
            in("rsi") top_of_stack.get() as u64,
            // We pass the target stack pointer in rdx.
            in("rdx") sp.get() as u64,

            // See switch_and_link() for an explanation of the clobbers.
            lateout("r12") _, lateout("r13") _, lateout("r14") _, lateout("r15") _,
            clobber_abi("sysv64"),
            options(may_unwind)
        }
    }

    (ret_val, StackPointer::new(ret_sp))
}

/// Drops the initial object on a coroutine that has not started yet.
#[inline]
pub unsafe fn drop_initial_obj(
    _stack_base: StackPointer,
    stack_ptr: StackPointer,
    drop_fn: unsafe fn(ptr: *mut u8),
) {
    // Safety: we stored the correct initial obj ptr here during stack initialization
    unsafe {
        let ptr = (stack_ptr.get() as *mut u8).add(8);
        drop_fn(ptr);
    }
}
