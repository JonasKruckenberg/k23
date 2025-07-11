// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::arch::asm;

pub const PAGE_SIZE: usize = 4096;

#[inline]
/// Returns the current stack pointer.
pub fn get_stack_pointer() -> usize {
    let stack_pointer: usize;
    // Safety: inline assembly below is safe, it just reads the stack pointer.
    unsafe {
        asm!(
            "mov {}, sp",
            out(reg) stack_pointer,
            options(nostack,nomem),
        );
    }
    stack_pointer
}

/// Retrieves the next older program counter and stack pointer from the current frame pointer.
/// The aarch64 calling conventions save the return PC one i64 above the FP and
/// the previous FP is pointed to by the current FP:
///
/// > Each frame shall link to the frame of its caller by means of a frame record
/// > of two 64-bit values on the stack [...] The frame record for the innermost
/// > frame [...] shall be pointed to by the frame pointer register (FP). The
/// > lowest addressed double-word shall point to the previous frame record and the
/// > highest addressed double-word shall contain the value passed in LR on entry
/// > to the current function.
///
/// - AAPCS64 section 6.2.3 The Frame Pointer[0]
pub unsafe fn get_next_older_pc_from_fp(fp: usize) -> usize {
    // Safety: caller has to ensure fp is valid
    let mut pc = unsafe { *(fp as *mut usize).offset(1) };

    // The return address might be signed, so we need to strip the highest bits
    // (where the authentication code might be located) in order to obtain a
    // valid address. We use the `XPACLRI` instruction, which is executed as a
    // no-op by processors that do not support pointer authentication, so that
    // the implementation is backward-compatible and there is no duplication.
    // However, this instruction requires the LR register for both its input and
    // output.
    unsafe {
        asm!(
        "mov lr, {pc}",
        "xpaclri",
        "mov {pc}, lr",
        pc = inout(reg) pc,
        out("lr") _,
        options(nomem, nostack, preserves_flags, pure),
        )
    };

    pc
}

/// The current frame pointer points to the next older frame pointer.
pub const NEXT_OLDER_FP_FROM_FP_OFFSET: usize = 0;

/// Asserts that the frame pointer is sufficiently aligned for the platform.
pub fn assert_fp_is_aligned(_fp: usize) {
    // From AAPCS64, section 6.2.3 The Frame Pointer[0]:
    //
    // > The location of the frame record within a stack frame is not specified.
    //
    // So this presumably means that the FP can have any alignment, as its
    // location is not specified and nothing further is said about constraining
    // alignment.
    //
    // [0]: https://github.com/ARM-software/abi-aa/blob/2022Q1/aapcs64/aapcs64.rst#the-frame-pointer
}
