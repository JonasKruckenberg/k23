// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::mem::{with_kernel_aspace, PageFaultFlags, VirtualAddress};
use crate::runtime::runtime;
use core::ops::ControlFlow;
use riscv::scause::{Exception, Trap};

pub fn handle_page_fault(trap: Trap, tval: VirtualAddress) -> ControlFlow<()> {
    let Some(scheduler) = runtime().local_scheduler() else {
        tracing::warn!("no currently active worker on this CPU");
        return ControlFlow::Continue(());
    };

    let Some(_current_task) = scheduler.current_task() else {
        // if we're not inside a task we're inside some critical kernel code
        // none of that should use ever trap
        tracing::warn!("no currently active task");
        return ControlFlow::Continue(());
    };

    let flags = match trap {
        Trap::Exception(Exception::LoadPageFault) => PageFaultFlags::LOAD,
        Trap::Exception(Exception::StorePageFault) => PageFaultFlags::STORE,
        Trap::Exception(Exception::InstructionPageFault) => PageFaultFlags::INSTRUCTION,
        // not a page fault exception, continue with the next fault handler
        _ => return ControlFlow::Continue(()),
    };

    // FIXME: bring back per-task address space pointers
    with_kernel_aspace(|aspace| {
        let mut aspace = aspace.lock();

        if let Err(err) = aspace.page_fault(tval, flags) {
            // the address space knew about the faulting address, but the requested access was invalid
            tracing::warn!("page fault handler couldn't correct fault {err}");
            ControlFlow::Continue(())
        } else {
            // the address space knew about the faulting address and could correct the fault
            tracing::trace!("page fault handler successfully corrected fault");
            ControlFlow::Break(())
        }
    })
}
