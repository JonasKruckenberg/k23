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
//! +--------------+
//! |              |
//! ~ Fiber-local  ~
//! |    data      |
//! +--------------+
//! |              |
//! ~     ...      ~
//! |              |
//! +--------------+
//! | Padding      |
//! +--------------+
//! | Saved PC     |
//! +--------------+
//! | Saved S1     |
//! +--------------+
//! | Saved S0     |
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
//! | Saved S1  |
//! +-----------+  <- The parent link points to here instead of pointing to the
//! | Saved PC  |     top of the stack. This matches the GCC/LLVM behavior of
//! +-----------+     having the frame pointer point to the address above the
//! | Saved S0  |     saved RA/FP.
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
//! +--------------+
//! |              |
//! ~ Fiber-local  ~
//! |    data      |
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
use riscv::{load_gp, save_gp, x, xlen_bytes};

pub const STACK_ALIGNMENT: usize = 16;

macro_rules! addi {
    ($dest:expr, $src:expr, $word_offset:expr) => {
        concat!("addi ", $dest, ", ", $src, ", ", xlen_bytes!($word_offset),)
    };
}

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

        // Add a 2-word offset because switch_and_link() looks for the target PC
        // 2 words above the stack pointer.
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
            // FIXME this is a workaround for bug in rustc/llvm
            //  https://github.com/rust-lang/rust/issues/80608#issuecomment-1094267279
            ".attribute arch, \"rv64gc\"",
            ".balign 4",
            ".cfi_startproc",
            // At this point our register state contains the following:
            // - SP points to the top of the parent stack.
            // - RA contains the return address in the parent context.
            // - S0 and S1 contain their value from the parent context.
            // - A2 points to the top of our stack.
            // - A1 points to the base of our stack.
            // - A0 contains the argument passed from switch_and_link.
            //
            // Push the S0, S1 and PC values of the parent context onto the parent
            // stack.
            // "addi sp, sp, -4 * 8",
            addi!("sp", "sp", -4),

            save_gp!(s1 => sp[2]),
            save_gp!(ra => sp[1]),
            save_gp!(s0 => sp[0]),
            // Write the parent stack pointer to the parent link. This is adjusted to
            // point just above the saved PC/RA to match the GCC/LLVM ABI.
            "addi t0, sp, 2 * 8",
            save_gp!(t0 => a1[-2]),
            // Set up the frame pointer to point at the stack base. This is needed for
            // the unwinding code below.
            "mv s0, a1",
            // Adjust A1 to point to the parent link.
            "addi a1, a1, -2 * 8",
            // Pop the padding and initial PC from the fiber stack. This also sets
            // up the 3rd argument to the initial function to point to the object that
            // init_stack() set up on the stack.
            "addi a2, a2, 4 * 8",

            // Switch to the fiber stack.
            "mv sp, a2",

            // Tell the unwinder where to find the Canonical Frame Address (CFA) of the
            // parent context.
            //
            // The CFA is normally defined as the stack pointer value in the caller just
            // before executing the call instruction. In our case, this is the stack
            // pointer value that should be restored upon exiting the inline assembly
            // block inside switch_and_link().
            ".cfi_escape 0x0f,  /* DW_CFA_def_cfa_expression */\
             5,                 /* the byte length of this expression */\
             0x78, 0x70,        /* DW_OP_breg8 (s0 - 8/16) */\
             0x06,              /* DW_OP_deref */\
             0x23, 2 * 8        /* DW_OP_plus_uconst 16 */",

            // Now we can tell the unwinder how to restore the 3 registers that were
            // pushed on the parent stack. These are described as offsets from the CFA
            // that we just calculated.
            ".cfi_offset s1, -2 * 8",
            ".cfi_offset ra, -3 * 8",
            ".cfi_offset s0, -4 * 8",

            // As in the original x86_64 code, hand-write the call operation so that it
            // doesn't push an entry into the CPU's return prediction stack.
            "lla ra, 0f",
            load_gp!(a1[1] => t0),
            "jr t0",

            "0:",
            // "unimp", // This UNIMP is necessary because of our use of .cfi_signal_frame earlier.
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
            load_gp!(a2[2] => t0),
            "jalr t0",

            // Upon returning, our register state contains the following:
            // - A2: Our stack pointer + 2 words.
            // - A1: The top of the fiber stack, or 0 if coming from
            //       switch_and_reset.
            // - A0: The argument passed from the fiber.

             // Switch back to our stack and free the saved registers.
            addi!("sp", "a2", 2),

            // Pass the argument in A0.
            inlateout("a0") arg0 => ret_val,
            // We get the fiber stack pointer back in A1.
            lateout("a1") ret_sp,
            // We pass the stack top in A1.
            in("a1") top_of_stack.get(),
            // We pass the target stack pointer in A2.
            in("a2") sp.get(),
            // Mark all registers as clobbered.
            lateout("s2") _, lateout("s3") _, lateout("s4") _, lateout("s5") _,
            lateout("s6") _, lateout("s7") _, lateout("s8") _, lateout("s9") _,
            lateout("s10") _, lateout("s11") _,
            lateout("fs0") _, lateout("fs1") _, lateout("fs2") _, lateout("fs3") _,
            lateout("fs4") _, lateout("fs5") _, lateout("fs6") _, lateout("fs7") _,
            lateout("fs8") _, lateout("fs9") _, lateout("fs10") _, lateout("fs11") _,
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
            // Save s0 and s1 while also reserving space on the stack for our
            // saved PC.
            addi!("sp", "sp", -4),
            save_gp!(s0 => sp[0]),
            save_gp!(s1 => sp[1]),
            // Write our return address to its expected position on the stack.
            "lla ra, 0f",
            save_gp!(ra => sp[2]),

            // Get the parent stack pointer from the parent link.
            load_gp!(t0[0] => a2),

            // Save our stack pointer to A1.
            "mv a1, sp",

            // Restore s0, s1 and ra from the parent stack.
            load_gp!(a2[0] => s1),
            load_gp!(a2[-1] => ra),
            load_gp!(a2[-2] => s0),

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
            // - RA contains the return address in the parent context.
            // - S0 and S1 contain their value from the parent context.
            // - A2 points to the top of our stack.
            // - A1 points to the base of our stack.
            // - A0 contains the argument passed from switch_and_link.
            "0:",

            // save s0, s1 and ra to the parent stack.
            addi!("sp", "sp", -4),
            save_gp!(s1 => sp[2]),
            save_gp!(ra => sp[1]),
            save_gp!(s0 => sp[0]),

            // now calculate the parent stack pointer, this points just above the saved ra
            // matching the GCC/LLVM ABI.
            addi!("t0", "sp", 2),

            // then save the parent stack pointer to the parent link "field" of our own stack
            save_gp!(t0 => a1[-2]),

            // Load our S0 and S1 values from the fiber stack.
            load_gp!(a2[1] => s1),
            load_gp!(a2[0] => s0),

            // Switch to the fiber stack while popping the saved registers and
            // padding.
            addi!("sp", "a2", 4),

            // Pass the argument in A0.
            inlateout("a0") arg => ret_val,
            // The parent link can be in any register, T0 is arbitrarily chosen
            // here.
            in("t0") parent_link,
            // See switch_and_link() for an explanation of the clobbers.
            lateout("s2") _, lateout("s3") _, lateout("s4") _, lateout("s5") _,
            lateout("s6") _, lateout("s7") _, lateout("s8") _, lateout("s9") _,
            lateout("s10") _, lateout("s11") _,
            lateout("fs0") _, lateout("fs1") _, lateout("fs2") _, lateout("fs3") _,
            lateout("fs4") _, lateout("fs5") _, lateout("fs6") _, lateout("fs7") _,
            lateout("fs8") _, lateout("fs9") _, lateout("fs10") _, lateout("fs11") _,
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
           "ld a2, 0({parent_link})",

            // Restore s0, s1 and ra from the parent stack.
            load_gp!(a2[0] => s1),
            load_gp!(a2[-1] => ra),
            load_gp!(a2[-2] => s0),

            // Return into the parent context
            "ret",

            in("a0") arg,
            // Hard-code the returned stack pointer value to 0 to indicate that this
            // fiber is done.
            in("a1") 0,
            parent_link = in(reg) parent_link,
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
                use panic_unwind2::resume_unwind;
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
            "lla ra, 0f",

            // Save the registers of the parent context.
            addi!("sp", "sp", -4),
            save_gp!(s1 => sp[2]),
            save_gp!(ra => sp[1]),
            save_gp!(s0 => sp[0]),

            // Write the parent stack pointer to the parent link. This is adjusted
            // to point just above the saved PC/RA to match the GCC/LLVM ABI.
            addi!("t1", "sp", 2),
            save_gp!(t1 => a1[-2]),

            // Load the coroutine registers, with the saved PC into RA.
            load_gp!(t0[2] => ra),
            load_gp!(t0[1] => s1),
            load_gp!(t0[0] => s0),

            // Switch to the coroutine stack while popping the saved registers and
            // padding.
            addi!("sp", "t0", 4),

            // Simulate a call with an artificial return address so that the throw
            // function will unwind straight into the switch_and_yield() call with
            // the register state expected outside the asm! block.
            "tail {throw}",

            // Upon returning, our register state is just like a normal return into
            // switch_and_link().
            "0:",

            // Switch back to our stack and free the saved registers.
            addi!("sp", "a2", 2),

            // Helper function to trigger stack unwinding.
            throw = sym throw,

            // Same output registers as switch_and_link().
            lateout("a0") ret_val,
            lateout("a1") ret_sp,

            // We pass the top of stack in a1.
            in("a1") top_of_stack.get() as u64,
            // We pass the target stack pointer in t0.
            in("t0") sp.get() as u64,

            // See switch_and_link() for an explanation of the clobbers.
            lateout("s2") _, lateout("s3") _, lateout("s4") _, lateout("s5") _,
            lateout("s6") _, lateout("s7") _, lateout("s8") _, lateout("s9") _,
            lateout("s10") _, lateout("s11") _,
            lateout("fs0") _, lateout("fs1") _, lateout("fs2") _, lateout("fs3") _,
            lateout("fs4") _, lateout("fs5") _, lateout("fs6") _, lateout("fs7") _,
            lateout("fs8") _, lateout("fs9") _, lateout("fs10") _, lateout("fs11") _,
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
        let ptr = (stack_ptr.get() as *mut u8).add(x!(16, 32));
        drop_fn(ptr);
    };
}
