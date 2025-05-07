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

        // Placeholders for returning TEB.StackLimit and TEB.GuaranteedStackBytes.
        push(&mut sp, None);
        push(&mut sp, None);

        // Initial function.
        push(&mut sp, Some(func as usize));

        // Placeholder for parent link.
        push(&mut sp, None);

        // Allocate space on the stack for the initial object, rounding to
        // STACK_ALIGNMENT.
        allocate_obj_on_stack(&mut sp, 32, obj);
        let init_obj = sp;

        // Write the TEB fields for the target stack.
        let teb = stack.teb_fields();
        push(&mut sp, Some(teb.GuaranteedStackBytes));
        push(&mut sp, Some(teb.StackBottomPlusGuard));
        push(&mut sp, Some(teb.StackBottom));
        push(&mut sp, Some(teb.StackTop));

        // The stack is aligned to STACK_ALIGNMENT at this point.
        debug_assert_eq!(sp % STACK_ALIGNMENT, 0);

        // Entry point called by switch_and_link().
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
            ".seh_proc stack_init_trampoline",
            // At this point our register state contains the following:
            // - RSP points to the top of the parent stack.
            // - RBP holds its value from the parent context.
            // - RDX points to the top of our stack.
            // - RSI points to the base of our stack.
            // - RDI contains the argument passed from switch_and_link.
            //
            // Save RBP from the parent context last to create a valid frame record.
            "push rbp",
            // Fill in the parent link.
            "mov [rsi - 32], rsp",
            // Adjust RSI to point to the parent link for the second argument.
            "sub rsi, 32",
            // Switch to the fiber stack, skipping the address of
            // stack_init_trampoline() at the top of the stack.
            "lea rsp, [rdx + 8]",
            // Reset the ExceptionList field of the TEB. This is not used on Win64 but
            // it *is* used by Wine. The end-of-chain value is !0.
            "mov qword ptr gs:[0x0], 0xffffffffffffffff",
            // Pop the TEB fields for our new stack.
            "pop qword ptr gs:[0x8]",    // StackBase
            "pop qword ptr gs:[0x10]",   // StackLimit
            "pop qword ptr gs:[0x1478]", // DeallocationStack
            "pop qword ptr gs:[0x1748]", // GuaranteedStackBytes
            // Set up the frame pointer to point at the parent link. This is needed for
            // the unwinding code below.
            "mov rbp, rsi",
            // These directives tell the unwinder how to restore the register state to
            // that of the parent call frame. These are executed in reverse by the
            // unwinder as it undoes the effects of the function prologue.
            //
            // When the unwinder reaches this frame, it will have a virtual RBP pointing
            // at the parent link. Then, the following SEH unwinding operations are
            // performed in order:
            // - .seh_setframe rbp, 0: This copies the virtual RBP to the virtual RSP so
            //   that all future stack references by the unwinder are done using that
            //   register.
            // - .seh_savereg rsp, 0: This is where the magic happens. The parent link
            //   is read off the stack and placed in the virtual RSP which now points to
            //   the top of the parent stack.
            // - .seh_pushreg rbp: This pops and restores RBP from the parent context.
            // - .seh_stackalloc 8: This increases RSP by 8 to pop the saved RIP.
            // - .seh_pushreg rbx: This pops and restores RBX from the parent context.
            // - .seh_stackalloc 40: This increases RSP by 40 to pop the saved TEB
            //   fields.
            //
            // After all these operations, the unwinder will pop a return address off
            // the stack. This is the secondary copy of the return address created in
            // switch_and_link.
            //
            // We end up with the register state that we had prior to entering the
            // asm!() block in switch_and_link. Unwinding can then proceed on the parent
            // stack normally.
            //
            // Note that we only use these unwind opcodes for computing backtraces. We
            // can't actually use this to throw an exception across stacks because the
            // unwinder will not update the TEB fields when switching stacks.
            ".seh_stackalloc 40",
            ".seh_pushreg rbx",
            ".seh_stackalloc 8",
            ".seh_pushreg rbp",
            ".seh_savereg rsp, 0",
            ".seh_setframe rbp, 0",
            ".seh_endprologue",
            // Set up the 3rd argument to the initial function to point to the object
            // that init_stack() set up on the stack.
            "mov rdx, rsp",
            // As in the original x86_64 code, hand-write the call operation so that it
            // doesn't push an entry into the CPU's return prediction stack.
            "lea rcx, [rip + 2f]",
            "push rcx",
            "jmp [rsi + 8]",
            "2:",
            // The SEH unwinder works by looking at a return target and scanning forward
            // to look for an epilog code sequence. We add an int3 instruction to avoid
            // this scan running off the end of the function. This works since int3 is
            // not part of any valid epilog code sequence.
            //
            // See LLVM's X86AvoidTrailingCall pass for more details:
            // https://github.com/llvm/llvm-project/blob/5aa24558cfa67e2a2e99c4e9c6d6b68bf372e00e/lib/Target/X86/X86AvoidTrailingCall.cpp
            "int3",
            ".seh_endproc",
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
            // Set up a secondary copy of the return address. This is only used by
            // the unwinder, not by actual returns.
            "lea rax, [rip + 2f]",
            "push rax",

            // Save the TEB fields to the stack.
            "push qword ptr gs:[0x1748]", // GuaranteedStackBytes
            "push qword ptr gs:[0x1478]", // DeallocationStack
            "push qword ptr gs:[0x10]", // StackLimit
            "push qword ptr gs:[0x8]", // StackBase
            "push qword ptr gs:[0x0]", // ExceptionList

            "push rbx",

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
            "2:",

            "pop rbx",

            // Restore the TEB fields.
            "pop qword ptr gs:[0x0]", // ExceptionList
            "pop qword ptr gs:[0x8]", // StackBase
            "pop qword ptr gs:[0x10]", // StackLimit
            "pop qword ptr gs:[0x1478]", // DeallocationStack
            "pop qword ptr gs:[0x1748]", // GuaranteedStackBytes

            // Pop the secondary return address.
            "add rsp, 8",

            // Pass the argument in RDI.
            inlateout("rdi") arg0 => ret_val,
            // We get the fiber stack pointer back in RSI.
            lateout("rsi") ret_sp,
            // We pass the top of stack in RSI.
            in("rsi") top_of_stack.get() as u64,
            // We pass the target stack pointer in RDX.
            in("rdx") sp.get() as u64,
            // Mark all registers as clobbered.
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
            // Save the TEB fields to the stack.
            "push qword ptr gs:[0x1748]", // GuaranteedStackBytes
            "push qword ptr gs:[0x1478]", // DeallocationStack
            "push qword ptr gs:[0x10]", // StackLimit
            "push qword ptr gs:[0x8]", // StackBase
            "push qword ptr gs:[0x0]", // ExceptionList

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

            // Restore the parent context's RBP.
            "pop rbp",

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

            // Save RBP from the parent context last to create a valid frame record.
            "push rbp",

            // Update the parent link near the base of the stack.
            "mov [rsi - 32], rsp",

            // Switch back to our stack, skipping the return address.
            "lea rsp, [rdx + 8]",

            "pop rbx",
            "pop rbp",

            // Restore the TEB fields.
            "pop qword ptr gs:[0x0]", // ExceptionList
            "pop qword ptr gs:[0x8]", // StackBase
            "pop qword ptr gs:[0x10]", // StackLimit
            "pop qword ptr gs:[0x1478]", // DeallocationStack
            "pop qword ptr gs:[0x1748]", // GuaranteedStackBytes

            // Pass the argument in RDI.
            inlateout("rdi") arg => ret_val,
            // The parent link can be in any register, RDX is arbitrarily chosen
            // here.
            in("rdx") parent_link as usize,
            // Mark all registers as clobbered.
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
            // Write the 2 TEB fields which can change during corountine execution
            // to the base of the stack. This is later recovered by
            // update_teb_from_stack().
            "mov rax, gs:[0x10]", // StackLimit
            "mov [rdx + 24], rax",
            "mov rax, gs:[0x1748]", // GuaranteedStackBytes
            "mov [rdx + 16], rax",

            // Load the parent context's stack pointer.
            "mov rsp, [rdx]",

            // Restore the parent context's RBP.
            "pop rbp",

            // Return into the parent context.
            "ret",

            in("rdx") parent_link as u64,
            in("rdi") arg,
            // Hard-code the returned stack pointer value to 0 to indicate that this
            // fiber is done.
            in("rsi") 0,
            options(noreturn),
        }
    }
}

#[inline]
pub unsafe fn switch_and_throw(
    sp: StackPointer,
    stack_base: StackPointer,
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
            // Save state just like the first half of switch_and_link().
            "lea rax, [rip + 2f]",
            "push rax",
            "push qword ptr gs:[0x1748]", // GuaranteedStackBytes
            "push qword ptr gs:[0x1478]", // DeallocationStack
            "push qword ptr gs:[0x10]", // StackLimit
            "push qword ptr gs:[0x8]", // StackBase
            "push qword ptr gs:[0x0]", // ExceptionList
            "push rbx",

            // Push a second copy of the return address to the stack.
            "push rax",

            // Save RBP of the parent context.
            "push rbp",

            // Update the parent link near the base of the coroutine stack.
            "mov [rsi - 32], rsp",

            // Switch to the coroutine stack.
            "mov rsp, rdx",

            // Pop the return address of the target context.
            "pop rax",

            // Restore RBP and RBX from the target context.
            "pop rbx",
            "pop rbp",

            // Restore the TEB fields of the target context.
            "pop qword ptr gs:[0x0]", // ExceptionList
            "pop qword ptr gs:[0x8]", // StackBase
            "pop qword ptr gs:[0x10]", // StackLimit
            "pop qword ptr gs:[0x1478]", // DeallocationStack
            "pop qword ptr gs:[0x1748]", // GuaranteedStackBytes

            // Simulate a call with an artificial return address so that the throw
            // function will unwind straight into the switch_and_yield() call with
            // the register state expected outside the asm! block.
            "push rax",
            "jmp {throw}",

            // Upon returning, our register state is just like a normal return into
            // switch_and_link().
            "2",

            // This is copied from the second half of switch_and_link().
            "pop rbx",
            "pop qword ptr gs:[0x0]", // ExceptionList
            "pop qword ptr gs:[0x8]", // StackBase
            "pop qword ptr gs:[0x10]", // StackLimit
            "pop qword ptr gs:[0x1478]", // DeallocationStack
            "pop qword ptr gs:[0x1748]", // GuaranteedStackBytes
            "add rsp, 8",

            // Helper function to trigger stack unwinding.
            throw = sym throw,

            // Argument to pass to the throw function.
            in("rdi") forced_unwind.0.get(),

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

#[inline]
pub unsafe fn drop_initial_obj(
    stack_base: StackPointer,
    stack_ptr: StackPointer,
    drop_fn: unsafe fn(ptr: *mut u8),
) {
    // Safety: we stored the correct values here during stack initialization
    unsafe {
        let ptr = (stack_ptr.get() as *mut u8).add(40);
        drop_fn(ptr);

        // Also copy the TEB fields to the base of the stack so that they can be
        // retrieved by update_stack_teb_fields().
        let base = stack_base.get() as *mut StackWord;
        let stack = stack_ptr.get() as *const StackWord;
        *base.sub(1) = *stack.add(2); // StackLimit
        *base.sub(2) = *stack.add(4); // GuaranteedStackBytes
    }
}

/// This function must be called after a stack has finished running a coroutine
/// so that the `StackLimit` and `GuaranteedStackBytes` fields from the TEB can
/// be updated in the stack. This is necessary if the stack is reused for
/// another coroutine.
#[inline]
pub unsafe fn update_stack_teb_fields(stack: &mut impl Stack) {
    let base = stack.base().get() as *const StackWord;
    let stack_limit = *base.sub(1) as usize;
    let guaranteed_stack_bytes = *base.sub(2) as usize;
    stack.update_teb_fields(stack_limit, guaranteed_stack_bytes);
}
