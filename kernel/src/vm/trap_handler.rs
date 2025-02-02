// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::error::Error;
use crate::traps::{Trap, TrapReason};
use crate::vm::{PageFaultFlags, KERNEL_ASPACE};
use core::ops::ControlFlow;

pub fn trap_handler(trap: Trap) -> ControlFlow<crate::Result<()>> {
    let mut aspace = KERNEL_ASPACE.get().unwrap().lock();

    let flags = match trap.reason {
        TrapReason::InstructionPageFault => PageFaultFlags::INSTRUCTION,
        TrapReason::LoadPageFault => PageFaultFlags::LOAD,
        TrapReason::StorePageFault => PageFaultFlags::STORE,
        _ => return ControlFlow::Continue(()),
    };

    if let Err(_err) = aspace.page_fault(trap.faulting_address, flags) {
        ControlFlow::Break(Err(Error::AccessDenied))
    } else {
        ControlFlow::Break(Ok(()))
    }
}
