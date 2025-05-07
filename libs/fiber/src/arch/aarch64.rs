// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! ## Stack layout
//!
//! Here is what the layout of the stack looks like when a coroutine is
//! suspended.
//!
//! ```text
//! +--------------+  <- Stack base
//! | Initial func |
//! +--------------+
//! | Parent link  |
//! +--------------+
//! |              |
//! ~     ...      ~
//! |              |
//! +--------------+
//! | Padding      |
//! +--------------+
//! | Saved PC     |
//! +--------------+
//! | Saved X29    |
//! +--------------+
//! | Saved X19    |
//! +--------------+
//! ```
//!
//! And this is the layout of the parent stack when a coroutine is running:
//!
//! ```text
//! |           |
//! ~    ...    ~
//! |           |
//! +-----------+
//! | Padding   |
//! +-----------+
//! | Saved X19 |
//! +-----------+
//! | Saved PC  |
//! +-----------+
//! | Saved X29 |
//! +-----------+
//! ```
//!
//! And finally, this is the stack layout of a coroutine that has just been
//! initialized:
//!
//! ```text
//! +--------------+  <- Stack base
//! | Initial func |
//! +--------------+
//! | Parent link  |
//! +--------------+
//! |              |
//! ~ Initial obj  ~
//! |              |
//! +--------------+
//! | Padding      |
//! +--------------+
//! | Initial PC   |
//! +--------------+
//! | Padding      |
//! +--------------+
//! | Padding      |
//! +--------------+  <- Initial stack pointer
//! ```

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

        // Initial function.
        push(&mut sp, Some(func as usize));

        // Placeholder for parent link.
        push(&mut sp, None);

        // Allocate space on the stack for the initial object, rounding to
        // STACK_ALIGNMENT.
        allocate_obj_on_stack(&mut sp, 16, obj);
        let init_obj = sp;

        // The stack is aligned to STACK_ALIGNMENT at this point.
        debug_assert_eq!(sp % STACK_ALIGNMENT, 0);

        // Padding so the final stack pointer value is properly aligned.
        push(&mut sp, None);

        // Entry point called by switch_and_link().
        push(&mut sp, Some(stack_init_trampoline as usize));

        // Add a 16-byte offset because switch_and_link() looks for the target PC
        // 16 bytes above the stack pointer.
        push(&mut sp, None);
        push(&mut sp, None);

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
            ".balign 4",
            ".cfi_startproc",
            // At this point our register state contains the following:
            // - SP points to the top of the parent stack.
            // - LR contains the return address in the parent context.
            // - X19 and X29 contain their value from the parent context.
            // - X2 points to the top of the fiber stack.
            // - X1 points to the base of our stack.
            // - X0 contains the argument passed from switch_and_link.
            //
            // Push the X19, X29 and PC values of the parent context onto the parent
            // stack.
            "stp x29, lr, [sp, #-32]!",
            "str x19, [sp, #16]",
            // Write the parent stack pointer to the parent link and adjust X1 to point
            // to the parent link.
            "mov x3, sp",
            "str x3, [x1, #-16]!",

            // Switch to the fiber stack and pop the padding and initial PC.
            "add sp, x2, #32",

            // Set up the frame pointer to point at the parent link. This is needed for
            // the unwinding code below.
            "mov x29, x1",

            // Tell the unwinder where to find the Canonical Frame Address (CFA) of the
            // parent context.
            //
            // The CFA is normally defined as the stack pointer value in the caller just
            // before executing the call instruction. In our case, this is the stack
            // pointer value that should be restored upon exiting the inline assembly
            // block inside switch_and_link().
            ".cfi_escape 0x0f,  /* DW_CFA_def_cfa_expression */\
            5,                  /* the byte length of this expression */\
            0x8d, 0x00,         /* DW_OP_breg29 (x29 + 0) */\
            0x06,               /* DW_OP_deref */\
            0x23, 0x20          /* DW_OP_plus_uconst 32 */",

            // Now we can tell the unwinder how to restore the 3 registers that were
            // pushed on the parent stack. These are described as offsets from the CFA
            // that we just calculated.
            ".cfi_offset x19, -16",
            ".cfi_offset lr, -24",
            ".cfi_offset x29, -32",

            // Set up the 3rd argument to the initial function to point to the object
            // that init_stack() set up on the stack.
            "mov x2, sp",

            // As in the original x86_64 code, hand-write the call operation so that it
            // doesn't push an entry into the CPU's return prediction stack.
            "adr lr, 0f",
            "ldr x3, [x1, #8]",
            "br x3",

            "0:",
            // // This BRK is necessary because of our use of .cfi_signal_frame earlier.
            // "brk #0",
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
            // DW_CFA_GNU_args_size 0
            //
            // Indicate to the unwinder that this "call" does not take any arguments
            // and no stack space needs to be popped before executing a landing pad.
            // This is mainly here to undo the effect of any previous
            // DW_CFA_GNU_args_size that may have been set in the current function.
            ".cfi_escape 0x2e, 0x00",

            // Read the saved PC from the fiber stack and call it.
            "ldr x3, [x2, #16]",
            "blr x3",

            // Upon returning, our register state contains the following:
            // - X2: Our stack pointer.
            // - X1: The top of the fiber stack, or 0 if coming from
            //       switch_and_reset.
            // - X0: The argument passed from the fiber.

            // Switch back to our stack and free the saved registers.
            "add sp, x2, #32",

            // Pass the argument in X0.
            inlateout("x0") arg0 => ret_val,
            // We get the fiber stack pointer back in X1.
            lateout("x1") ret_sp,
            // We pass the top of stack in X1.
            in("x1") top_of_stack.get() as u64,
            // We pass the target stack pointer in X3.
            in("x2") sp.get() as u64,
            // Mark all registers as clobbered. The clobber_abi() will automatically
            // mark X18 as clobbered if it is not reserved by the platform.
            lateout("x20") _, lateout("x21") _, lateout("x22") _, lateout("x23") _,
            lateout("x24") _, lateout("x25") _, lateout("x26") _, lateout("x27") _,
            lateout("x28") _,
            clobber_abi("C"),
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
            // Save X19 and X29 while also reserving space on the stack for our
            // saved PC.
            "stp x19, x29, [sp, #-32]!",

            // Write our return address to its expected position on the stack.
            "adr lr, 0f",
            "str lr, [sp, #16]",

            // Get the parent stack pointer from the parent link.
            "ldr x2, [x2]",

            // Save our stack pointer to X1.
            "mov x1, sp",

            // Restore X19, X29 and LR from the parent stack.
            "ldr x19, [x2, #16]",
            "ldp x29, lr, [x2]",

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

            // Return into the parent context
            "ret",

            // This gets called by switch_and_link(). At this point our register
            // state contains the following:
            // - SP points to the top of the parent stack.
            // - LR contains the return address in the parent context.
            // - X19 and X29 contain their value from the parent context.
            // - X2 points to the top of the fiber stack.
            // - X1 points to the base of our stack.
            // - X0 contains the argument passed from switch_and_link.
            "0:",

            // Push the X19, X29 and PC values of the parent context onto the parent
            // stack.
            "stp x29, lr, [sp, #-32]!",
            "str x19, [sp, #16]",

            // Write the parent stack pointer to the parent link.
            "mov x3, sp",
            "str x3, [x1, #-16]",

            // Load our X19 and X29 values from the fiber stack.
            "ldp x19, x29, [x2]",

            // Switch to the fiber stack while popping the saved registers and
            // padding.
            "add sp, x2, #32",

            // Pass the argument in X0.
            inlateout("x0") arg => ret_val,
            // The parent link can be in any register, X2 is arbitrarily chosen
            // here.
            in("x2") parent_link as u64,
            // See switch_and_link() for an explanation of the clobbers.
            lateout("x20") _, lateout("x21") _, lateout("x22") _, lateout("x23") _,
            lateout("x24") _, lateout("x25") _, lateout("x26") _, lateout("x27") _,
            lateout("x28") _,
            clobber_abi("C"),
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
            "ldr x2, [{parent_link}]",

            // Restore X19, X29 and LR from the parent stack.
            "ldr x19, [x2, #16]",
            "ldp x29, lr, [x2]",

            // Return into the parent context
            "ret",

            parent_link = in(reg) parent_link as u64,
            in("x0") arg,
            // Hard-code the returned stack pointer value to 0 to indicate that this
            // fiber is done.
            in("x1") 0,
            options(noreturn),
        }
    }
}

#[inline]
pub unsafe fn switch_and_throw(
    sp: StackPointer,
    top_of_stack: StackPointer,
) -> (EncodedValue, Option<StackPointer>) {
    extern "C-unwind" fn throw() -> ! {
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
            // Set up a return address.
            "adr lr, 0f",

            // Save the registers of the parent context.
            "stp x29, lr, [sp, #-32]!",
            "str x19, [sp, #16]",

            // Update the parent link near the base of the coroutine stack.
            "mov x3, sp",
            "str x3, [x1, #-16]",

            // Load the coroutine registers, with the saved PC into LR.
            "ldr lr, [x2, #16]",
            "ldp x19, x29, [x2]",

            // Switch to the coroutine stack while popping the saved registers and
            // padding.
            "add sp, x2, #32",

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
            "b {throw}",

            // Upon returning, our register state is just like a normal return into
            // switch_and_link().
            "0:",

            // Switch back to our stack and free the saved registers.
            "add sp, x2, #32",

            // Helper function to trigger stack unwinding.
            throw = sym throw,

            // Same output registers as switch_and_link().
            lateout("x0") ret_val,
            lateout("x1") ret_sp,

            // We pass the top of stack in X1.
            in("x1") top_of_stack.get() as u64,
            // We pass the target stack pointer in X3.
            in("x2") sp.get() as u64,

            // See switch_and_link() for an explanation of the clobbers.
            lateout("x20") _, lateout("x21") _, lateout("x22") _, lateout("x23") _,
            lateout("x24") _, lateout("x25") _, lateout("x26") _, lateout("x27") _,
            lateout("x28") _,
            clobber_abi("C"),
            options(may_unwind)
        }
    }

    (ret_val, StackPointer::new(ret_sp))
}

#[inline]
pub unsafe fn drop_initial_obj(
    _stack_base: StackPointer,
    stack_ptr: StackPointer,
    drop_fn: unsafe fn(ptr: *mut u8),
) {
    // Safety: we stored the correct initial obj ptr here during stack initialization
    unsafe {
        let ptr = (stack_ptr.get() as *mut u8).add(32);
        drop_fn(ptr);
    }
}
