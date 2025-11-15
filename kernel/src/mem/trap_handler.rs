// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ops::ControlFlow;

use kmem_core::VirtualAddress;
use riscv::scause::Exception;

use crate::arch::trap::Trap;
use crate::mem::{PageFaultFlags, with_kernel_aspace};

pub fn handle_page_fault(trap: Trap, tval: VirtualAddress) -> ControlFlow<()> {
    let flags = match trap {
        Trap::Exception(Exception::LoadPageFault) => PageFaultFlags::LOAD,
        Trap::Exception(Exception::StorePageFault) => PageFaultFlags::STORE,
        Trap::Exception(Exception::InstructionPageFault) => PageFaultFlags::INSTRUCTION,
        _ => return ControlFlow::Continue(()),
    };

    // For now, use kernel address space for page faults
    // WASM tests run in kernel context, so this should work for our current needs
    // TODO: In the future, tasks should carry their own address space as metadata
    with_kernel_aspace(|aspace| {
        let mut aspace = aspace.lock();
        if let Err(err) = aspace.page_fault(tval, flags) {
            tracing::warn!("page fault handler couldn't correct fault {err}");
            ControlFlow::Continue(())
        } else {
            tracing::trace!("page fault handler successfully corrected fault");
            ControlFlow::Break(())
        }
    })
}
