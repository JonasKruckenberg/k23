// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::utils::{define_op, load_fp, load_gp, save_fp, save_gp};
use crate::arch::PAGE_SIZE;
use crate::backtrace::Backtrace;
use crate::error::Error;
use crate::scheduler::scheduler;
use crate::vm::{PageFaultFlags, VirtualAddress};
use crate::{arch, panic, TRAP_STACK_SIZE_PAGES};
use alloc::boxed::Box;
use alloc::string::ToString;
use arrayvec::ArrayVec;
use core::arch::{asm, naked_asm};
use core::ops::ControlFlow;
use cpu_local::cpu_local;
use riscv::scause::{Exception, Interrupt, Trap};
use riscv::sstatus::SPP;
use riscv::{sbi, scause, sepc, sip, sscratch, sstatus, stval, stvec};

cpu_local! {
    static TRAP_STACK: [u8; TRAP_STACK_SIZE_PAGES * PAGE_SIZE] = const { [0; TRAP_STACK_SIZE_PAGES * PAGE_SIZE] };
}

pub fn init() {
    // Safety: this is fine
    let trap_stack_top = unsafe {
        TRAP_STACK
            .as_ptr()
            .byte_add(TRAP_STACK_SIZE_PAGES * PAGE_SIZE) as *mut u8
    };

    tracing::trace!("setting sscratch to {:p}", trap_stack_top);
    // Safety: inline assembly
    unsafe {
        asm!(
            "csrrw x0, sscratch, {trap_frame}", // sscratch points to the trap frame
            trap_frame = in(reg) trap_stack_top
        );
    }

    tracing::trace!("setting trap vec to {:#x}", trap_vec as usize);
    // Safety: register access
    unsafe { stvec::write(trap_vec as usize, stvec::Mode::Vectored) };
}

#[repr(C)]
#[derive(Clone, Default)]
pub struct TrapFrame {
    pub gp: [usize; 32],
    #[cfg(target_feature = "d")]
    pub fp: [usize; 32],
}

#[naked]
unsafe extern "C" fn trap_vec() {
    // When in vectored mode
    // exceptions i.e. sync traps => BASE
    // interrupts i.e. async traps => BASE + 4 * CAUSE
    //
    // We can use this to direct some traps that don't need
    // expensive SBI call handling to cheaper handlers (like timers)
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            ".align 2",
            ".option push",
            ".option norvc",
            "j {default}", // exception
            "j {default}", // supervisor software interrupt
            "j {default}", // reserved
            "j {default}", // reserved
            "j {default}", // reserved
            "j {default}", // supervisor timer interrupt
            "j {default}", // reserved
            "j {default}", // reserved
            "j {default}", // reserved
            "j {default}", // supervisor external interrupt
            ".option pop",
            default = sym default_trap_entry,
        }
    }
}

#[naked]
unsafe extern "C" fn default_trap_entry() {
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            // FIXME this is a workaround for bug in rustc/llvm
            //  https://github.com/rust-lang/rust/issues/80608#issuecomment-1094267279
            ".align 2",
            ".attribute arch, \"rv64gc\"",
            ".cfi_startproc",

            // Set the CFI rule for the return address to always return zero
            // This is always the first frame on stack, there is nowhere to return to
            ".cfi_register ra, zero",

            "csrrw sp, sscratch, sp", // sp points to the TrapFrame

            "add sp, sp, -0x210",
            ".cfi_def_cfa_offset 0x210",

            // save gp regs
            save_gp!(x0 => sp[0]),
            save_gp!(x1 => sp[1]),
            // skip sp since it is saved in sscratch
            save_gp!(x3 => sp[3]),
            save_gp!(x4 => sp[4]),
            save_gp!(x5 => sp[5]),
            save_gp!(x6 => sp[6]),
            save_gp!(x7 => sp[7]),
            save_gp!(x8 => sp[8]),
            save_gp!(x9 => sp[9]),
            save_gp!(x10 => sp[10]),
            save_gp!(x11 => sp[11]),
            save_gp!(x12 => sp[12]),
            save_gp!(x13 => sp[13]),
            save_gp!(x14 => sp[14]),
            save_gp!(x15 => sp[15]),
            save_gp!(x16 => sp[16]),
            save_gp!(x17 => sp[17]),
            save_gp!(x18 => sp[18]),
            save_gp!(x19 => sp[19]),
            save_gp!(x20 => sp[20]),
            save_gp!(x21 => sp[21]),
            save_gp!(x22 => sp[22]),
            save_gp!(x23 => sp[23]),
            save_gp!(x24 => sp[24]),
            save_gp!(x25 => sp[25]),
            save_gp!(x26 => sp[26]),
            save_gp!(x27 => sp[27]),
            save_gp!(x28 => sp[28]),
            save_gp!(x29 => sp[29]),
            save_gp!(x30 => sp[30]),
            save_gp!(x31 => sp[31]),

            // save fp regs
            save_fp!(f0 => sp[32]),
            save_fp!(f1 => sp[33]),
            save_fp!(f2 => sp[34]),
            save_fp!(f3 => sp[35]),
            save_fp!(f4 => sp[36]),
            save_fp!(f5 => sp[37]),
            save_fp!(f6 => sp[38]),
            save_fp!(f7 => sp[39]),
            save_fp!(f8 => sp[40]),
            save_fp!(f9 => sp[41]),
            save_fp!(f10 => sp[42]),
            save_fp!(f11 => sp[43]),
            save_fp!(f12 => sp[44]),
            save_fp!(f13 => sp[45]),
            save_fp!(f14 => sp[46]),
            save_fp!(f15 => sp[47]),
            save_fp!(f16 => sp[48]),
            save_fp!(f17 => sp[49]),
            save_fp!(f18 => sp[50]),
            save_fp!(f19 => sp[51]),
            save_fp!(f20 => sp[52]),
            save_fp!(f21 => sp[53]),
            save_fp!(f22 => sp[54]),
            save_fp!(f23 => sp[55]),
            save_fp!(f24 => sp[56]),
            save_fp!(f25 => sp[57]),
            save_fp!(f26 => sp[58]),
            save_fp!(f27 => sp[59]),
            save_fp!(f28 => sp[60]),
            save_fp!(f29 => sp[61]),
            save_fp!(f30 => sp[62]),
            save_fp!(f31 => sp[63]),

            "mv a0, sp",

            "call {trap_handler}",

            "mv sp, a0",

            // restore gp regs
            // skip x0 since its always zero
            load_gp!(sp[1] => x1),
            // skip sp since it is saved in sscratch
            load_gp!(sp[3] => x3),
            load_gp!(sp[4] => x4),
            load_gp!(sp[5] => x5),
            load_gp!(sp[6] => x6),
            load_gp!(sp[7] => x7),
            load_gp!(sp[8] => x8),
            load_gp!(sp[9] => x9),
            load_gp!(sp[10] => x10),
            load_gp!(sp[11] => x11),
            load_gp!(sp[12] => x12),
            load_gp!(sp[13] => x13),
            load_gp!(sp[14] => x14),
            load_gp!(sp[15] => x15),
            load_gp!(sp[16] => x16),
            load_gp!(sp[17] => x17),
            load_gp!(sp[18] => x18),
            load_gp!(sp[19] => x19),
            load_gp!(sp[20] => x20),
            load_gp!(sp[21] => x21),
            load_gp!(sp[22] => x22),
            load_gp!(sp[23] => x23),
            load_gp!(sp[24] => x24),
            load_gp!(sp[25] => x25),
            load_gp!(sp[26] => x26),
            load_gp!(sp[27] => x27),
            load_gp!(sp[28] => x28),
            load_gp!(sp[29] => x29),
            load_gp!(sp[30] => x30),
            load_gp!(sp[31] => x31),

            // restore fp regs
            load_fp!(sp[32] => f0),
            load_fp!(sp[33] => f1),
            load_fp!(sp[34] => f2),
            load_fp!(sp[35] => f3),
            load_fp!(sp[36] => f4),
            load_fp!(sp[37] => f5),
            load_fp!(sp[38] => f6),
            load_fp!(sp[39] => f7),
            load_fp!(sp[40] => f8),
            load_fp!(sp[41] => f9),
            load_fp!(sp[42] => f10),
            load_fp!(sp[43] => f11),
            load_fp!(sp[44] => f12),
            load_fp!(sp[45] => f13),
            load_fp!(sp[46] => f14),
            load_fp!(sp[47] => f15),
            load_fp!(sp[48] => f16),
            load_fp!(sp[49] => f17),
            load_fp!(sp[50] => f18),
            load_fp!(sp[51] => f19),
            load_fp!(sp[52] => f20),
            load_fp!(sp[53] => f21),
            load_fp!(sp[54] => f22),
            load_fp!(sp[55] => f23),
            load_fp!(sp[56] => f24),
            load_fp!(sp[57] => f25),
            load_fp!(sp[58] => f26),
            load_fp!(sp[59] => f27),
            load_fp!(sp[60] => f28),
            load_fp!(sp[61] => f29),
            load_fp!(sp[62] => f30),
            load_fp!(sp[63] => f31),

            "add sp, sp, 0x210",
            ".cfi_def_cfa_offset 0",

            "csrrw sp, sscratch, sp",
            "sret",
            ".cfi_endproc",

            trap_handler = sym default_trap_handler,
        }
    }
}

// https://github.com/emb-riscv/specs-markdown/blob/develop/exceptions-and-interrupts.md
#[expect(clippy::too_many_arguments, reason = "")]
extern "C" fn default_trap_handler(
    raw_frame: *mut TrapFrame,
    _a1: usize,
    _a2: usize,
    _a3: usize,
    _a4: usize,
    _a5: usize,
    _a6: usize,
    _a7: usize,
) -> *mut TrapFrame {
    let cause = scause::read().cause();

    let epc = sepc::read();
    let tval = VirtualAddress::new(stval::read()).unwrap();
    tracing::trace!("{cause:?} {:?};epc={epc:#x};tval={tval}", sstatus::read());

    match cause {
        Trap::Interrupt(Interrupt::SupervisorSoft) => {
            // Just a nop, software interrupts are only used as wakeup calls
            // TODO this should be an specialized routine in the trap vector

            // Safety: register access
            unsafe {
                sip::clear_ssoft();
            }

            raw_frame
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            if let (_, Some(next_deadline)) = scheduler().cpu_local_timer().turn() {
                // Timer interrupts are always IPIs used for sleeping
                sbi::time::set_timer(next_deadline.ticks.0).unwrap();
            } else {
                // Timer interrupts are always IPIs used for sleeping
                sbi::time::set_timer(u64::MAX).unwrap();
            }

            raw_frame
        }
        Trap::Interrupt(Interrupt::SupervisorExternal) => todo!("run IO reactor"),
        Trap::Exception(Exception::LoadPageFault)
        | Trap::Exception(Exception::StorePageFault)
        | Trap::Exception(Exception::InstructionPageFault) => {
            // first attempt the page fault handler, can it recover us from this by fixing up mappings?
            if handle_page_fault(cause, tval).is_break() {
                return raw_frame;
            }

            // if not attempt the wasm fault handler, is the current trap caused by a user program?
            // if so can it kill the program?
            if handle_wasm_exception().is_break() {
                return raw_frame;
            }

            handle_kernel_exception(cause, raw_frame, epc, tval)
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            if handle_wasm_exception().is_break() {
                return raw_frame;
            }

            handle_kernel_exception(cause, raw_frame, epc, tval)
        }
        _ => handle_kernel_exception(cause, raw_frame, epc, tval),
    }
}

fn handle_page_fault(trap: Trap, tval: VirtualAddress) -> ControlFlow<()> {
    // crate::vm::with_current_aspace(|aspace| {
    //     let flags = match trap {
    //         Trap::Exception(Exception::LoadPageFault) => PageFaultFlags::LOAD,
    //         Trap::Exception(Exception::StorePageFault) => PageFaultFlags::STORE,
    //         Trap::Exception(Exception::InstructionPageFault) => PageFaultFlags::INSTRUCTION,
    //         // not a page fault exception, continue with the next fault handler
    //         _ => return ControlFlow::Continue(())
    //     };
    //
    //     if let Err(_err) = aspace.page_fault(tval, flags) {
    //         // the address space knew about the faulting address, but the requested access was invalid
    //         ControlFlow::Continue(())
    //     } else {
    //         // the address space knew about the faulting address and could correct the fault
    //         ControlFlow::Break(())
    //     }
    // })

    // TODO find the CURRENT address space

    ControlFlow::Continue(())
}

fn handle_wasm_exception() -> ControlFlow<()> {
    // TODO The trap occurred while executing a WASM program
    //      => capture backtrace
    //      => unwind stack to next rust frame (before all the WASM frames)

    ControlFlow::Continue(())
}

fn handle_kernel_exception(
    cause: Trap,
    frame: *mut TrapFrame,
    epc: usize,
    tval: VirtualAddress,
) -> ! {
    // let's go ahead and begin unwinding the stack that caused the fault
    // Note: we use unwinding here to give kernel code the chance to catch this and recover from it.
    // If the unwinding reaches the root `catch_unwind` in `main.rs` this will tear down the entire
    // system causing all CPUs to shut down.

    tracing::error!("KERNEL TRAP {cause:?} epc={epc:#x};tval={tval}");

    let frame = unsafe { &*frame };
    let mut regs = unwind2::Registers {
        gp: frame.gp,
        fp: frame.fp,
    };
    regs.gp[2] = sscratch::read();

    let backtrace = Backtrace::<32>::from_registers(regs.clone(), epc + 1).unwrap();
    tracing::error!("{backtrace}");

    // FIXME it would be great to get rid of the allocation here :/
    let payload = Box::new(cause);

    // begin a panic on the original stack
    panic::begin_unwind_with(payload, regs, epc + 1);
}
