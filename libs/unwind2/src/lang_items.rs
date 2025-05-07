// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::exception::Exception;
use crate::utils::with_context;
use crate::{Error, FrameIter, arch, raise_exception_phase2};
use abort::abort;

/// In traditional unwinders the personality routine is responsible for determining the unwinders
/// behaviour for each frame (stop unwinding because a handler has been found, continue etc.)
/// Since `unwind2` only cares about Rust code, the personality routine here is just a stub to make
/// the compiler happy and ensure we're not unwinding across language boundaries. The real unwinding
/// happens in [`raise_exception_phase2`].
#[lang = "eh_personality"]
extern "C" fn personality_stub() {}

/// Ensure the ptr points to the expected personality routine stub.
pub fn ensure_personality_stub(ptr: u64) -> crate::Result<()> {
    if ptr == personality_stub as usize as u64 {
        Ok(())
    } else {
        Err(Error::DifferentPersonality)
    }
}

/// Rust generated landing pads for `Drop` cleanups end with calls to `_Unwind_Resume`
/// to continue unwinding the stack. This is what transfers control back to the unwinder.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn _Unwind_Resume(exception: *mut Exception) -> ! {
    with_context(|regs, pc| {
        let frames = FrameIter::from_registers(regs.clone(), pc);

        match raise_exception_phase2(frames, exception) {
            Ok(_) => {}
            Err(Error::EndOfStack) => {
                tracing::error!("Uncaught exception");
                abort();
            }
            Err(err) => {
                tracing::error!("Failed to resume exception: {err:?}");
                abort()
            }
        }

        // Safety: this replaces the register state, very unsafe
        unsafe { arch::restore_context(regs) }
    })
}
