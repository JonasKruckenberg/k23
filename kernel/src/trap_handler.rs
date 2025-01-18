// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::longjmp;
use crate::vm::VirtualAddress;
use crate::{arch, vm};
use core::cell::Cell;
use core::fmt::Write;
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::ControlFlow;
use core::ptr;
use core::ptr::addr_of_mut;
use thread_local::thread_local;

thread_local! {
    static TRAP_RESUME_STATE: Cell<*mut TrapResumeState> = Cell::new(ptr::null_mut());
    static IN_TRAP_HANDLER: Cell<bool> = Cell::new(false);
}

#[derive(Debug, Copy, Clone)]
pub struct Trap {
    pub pc: VirtualAddress,
    pub fp: VirtualAddress,
    pub faulting_address: VirtualAddress,
    pub reason: TrapReason,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub enum TrapReason {
    SupervisorSoftwareInterrupt,
    SupervisorTimerInterrupt,
    SupervisorExternalInterrupt,

    /// Instruction address misaligned
    InstructionMisaligned,
    /// Instruction access fault
    InstructionFault,
    /// Illegal instruction
    IllegalInstruction,
    /// Breakpoint
    Breakpoint,

    /// Load address misaligned
    LoadMisaligned,
    /// Load fault
    LoadFault,
    /// Store address misaligned
    StoreMisaligned,
    /// Store fault
    StoreFault,

    /// Instruction page fault
    InstructionPageFault,
    /// Load page fault
    LoadPageFault,
    /// Store/AMO page fault
    StorePageFault,

    /// Environment call
    EnvCall,
}

struct TrapResumeState {
    catch_fn: fn(*mut u8, Trap),
    data_ptr: *mut u8,
    prev_state: *mut TrapResumeState,
    jmp_buf: arch::JmpBuf,
}

/// Raises a trap on the current hart without triggering subsystem page fault routines (i.e. faulting
/// in pages).
pub fn resume_trap(trap: Trap) -> ! {
    IN_TRAP_HANDLER.set(false);

    let data = TRAP_RESUME_STATE.get();
    if data.is_null() {
        // If data is null that means we encountered a trap without any `catch_traps`. So just
        // delegate to the default resume function which just panics
        fault_resume_panic(trap.reason, trap.pc, trap.fp, trap.faulting_address);
    } else {
        // Safety: If the data pointer is not null, it must point to some `TrapResumeState` struct
        // so all fields are initialized and valid
        unsafe {
            let data = &*data;

            (data.catch_fn)(data.data_ptr, trap);

            TRAP_RESUME_STATE.set(data.prev_state);

            longjmp(data.jmp_buf, 1);
        }
    }
}

/// Invokes a closure, capturing information about a hardware trap if one occurs.
///
/// Analogous to [`catch_unwind`][1] this will return `Ok` with the closures
/// result if the closure didn't trigger a trap, and will return `Err(trap)` if it did. The `trap` object
/// holds further information about the traps instruction pointer, faulting address and trap reason.
///
/// [1]: [crate::panic::catch_unwind]
pub fn catch_traps<F, R>(f: F) -> Result<R, Trap>
where
    F: FnOnce() -> R,
{
    union Data<R> {
        // when the closure completed successfully, this will hold the return
        r: ManuallyDrop<R>,
        // when the closure panicked this will hold the panic payload
        p: ManuallyDrop<Trap>,
    }

    #[cold]
    fn do_catch<R>(data: *mut u8, trap: Trap) {
        let data = data.cast::<Data<R>>();
        // Safety: data is correctly initialized
        let data = unsafe { &mut (*data) };
        data.p = ManuallyDrop::new(trap);
    }

    let mut data: MaybeUninit<Data<R>> = MaybeUninit::zeroed();
    let data_ptr = addr_of_mut!(data).cast::<u8>();

    let ret_code = arch::call_with_setjmp(|jmp_buf| {
        let mut state = TrapResumeState {
            catch_fn: do_catch::<R>,
            data_ptr,
            prev_state: TRAP_RESUME_STATE.get(),
            jmp_buf: ptr::from_ref(jmp_buf),
        };
        let prev_state = TRAP_RESUME_STATE.replace(ptr::from_mut(&mut state).cast());

        f();

        TRAP_RESUME_STATE.set(prev_state);

        0_i32
    });

    // Safety: union access
    unsafe {
        if ret_code == 0 {
            Ok(ManuallyDrop::into_inner(data.assume_init().r))
        } else {
            Err(ManuallyDrop::into_inner(data.assume_init().p))
        }
    }
}

fn fault_resume_panic(
    reason: TrapReason,
    pc: VirtualAddress,
    fp: VirtualAddress,
    faulting_address: VirtualAddress,
) -> ! {
    panic!("UNCAUGHT KERNEL TRAP {reason:?} pc={pc};fp={fp};faulting_address={faulting_address};");
}

/// Begins processing a trap.
///
/// Contrary to `resume_trap` this function will trigger all subsystem trap
/// handlers and is expected to be called from the architecture specific trap handler.
pub fn begin_trap(trap: Trap) {
    if IN_TRAP_HANDLER.replace(true) {
        let _ = riscv::hio::HostStream::new_stdout()
            .write_str("trap occurred while in trap handler!\n");
        arch::abort();
    }

    // Consult the vm subsystem trap handler, does it have special handling?
    // If it does, it will return a `ControlFlow::Break` with the result of the trap handler.
    // If it doesn't, it will return `ControlFlow::Continue` and we will continue with the default
    // behaviour (i.e. resuming the trap).
    //
    // Note that the trap handler also might break with an error indicating that the trap handler
    // *did* have special handling but that logic says not to continue with program execution.
    if let ControlFlow::Break(res) = vm::trap_handler(trap) {
        if let Err(err) = res {
            log::error!("error in vm trap handler {err:?}");
            resume_trap(trap);
        } else {
            return;
        }
    }

    resume_trap(trap);
}
