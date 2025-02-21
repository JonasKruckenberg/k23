// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::exception::Exception;
use crate::utils::with_context;
use crate::{arch, raise_exception_phase2, Error, FramesIter};

#[lang = "eh_personality"]
extern "C" fn personality_stub() {}

pub fn ensure_personality_stub(ptr: u64) -> crate::Result<()> {
    if ptr == personality_stub as usize as u64 {
        Ok(())
    } else {
        Err(Error::DifferentPersonality)
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn _Unwind_Resume(exception: *mut Exception) -> ! {
    with_context(|regs, pc| {
        let frames = FramesIter::from_registers(regs.clone(), pc);

        if let Err(err) = raise_exception_phase2(frames, exception) {
            log::error!("Failed to resume exception: {err:?}");
            arch::abort("Failed to resume exception")
        }

        // Safety: this replaces the register state, very unsafe
        unsafe { arch::restore_context(regs) }
    })
}
