// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#[inline]
/// Returns the current stack pointer.
pub fn get_stack_pointer() -> usize {
    let stack_pointer: usize;
    unsafe {
        core::arch::asm!(
        "mov {}, rsp",
        out(reg) stack_pointer,
        options(nostack,nomem),
        );
    }
    stack_pointer
}

/// Retrieves the next older program counter and stack pointer from the current frame pointer.
pub unsafe fn get_next_older_pc_from_fp(fp: usize) -> usize {
    // The calling convention always pushes the return pointer (aka the PC of
    // the next older frame) just before this frame.
    *(fp as *mut usize).offset(1)
}

/// The current frame pointer points to the next older frame pointer.
pub const NEXT_OLDER_FP_FROM_FP_OFFSET: usize = 0;

/// Asserts that the frame pointer is sufficiently aligned for the platform.
pub fn assert_fp_is_aligned(fp: usize) {
    assert_eq!(fp % 16, 0, "stack should always be aligned to 16");
}
