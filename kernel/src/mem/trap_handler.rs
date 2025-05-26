// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::mem::VirtualAddress;
use crate::state::global;
use async_exec::scheduler::Schedule;
use core::ops::ControlFlow;
use riscv::scause::Trap;

pub fn handle_page_fault(_trap: Trap, _tval: VirtualAddress) -> ControlFlow<()> {
    let current_task = global().executor.cpu_local_scheduler().current_task();

    let Some(_current_task) = current_task.as_ref() else {
        // if we're not inside a task we're inside some critical kernel code
        // none of that should use ever trap
        tracing::warn!("no currently active task");
        return ControlFlow::Continue(());
    };

    todo!()

    // let mut aspace = current_task.header().aspace.as_ref().unwrap().lock();

    // let flags = match trap {
    //     Trap::Exception(Exception::LoadPageFault) => PageFaultFlags::LOAD,
    //     Trap::Exception(Exception::StorePageFault) => PageFaultFlags::STORE,
    //     Trap::Exception(Exception::InstructionPageFault) => PageFaultFlags::INSTRUCTION,
    //     // not a page fault exception, continue with the next fault handler
    //     _ => return ControlFlow::Continue(()),
    // };
    //
    // if let Err(err) = aspace.page_fault(tval, flags) {
    //     // the address space knew about the faulting address, but the requested access was invalid
    //     tracing::warn!("page fault handler couldn't correct fault {err}");
    //     ControlFlow::Continue(())
    // } else {
    //     // the address space knew about the faulting address and could correct the fault
    //     tracing::trace!("page fault handler successfully corrected fault");
    //     ControlFlow::Break(())
    // }
}
