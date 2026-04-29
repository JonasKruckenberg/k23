// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::arch::naked_asm;
use core::cell::Cell;
use core::ops::ControlFlow;

use gimli::RiscV;
use kcpu_local::cpu_local;
use kmem_core::VirtualAddress;
use riscv::scause::{Exception, Interrupt};
use riscv::{
    load_fp, load_gp, save_fp, save_gp, scause, sepc, sip, sscratch, sstatus, stval, stvec,
};

use crate::arch::PAGE_SIZE;
use crate::arch::trap::Trap;
use crate::backtrace::Backtrace;
use crate::state::{cpu_local, global};
use crate::{TRAP_STACK_SIZE_PAGES, irq};

cpu_local! {
    static IN_TRAP: Cell<bool> = Cell::new(false);
    static TRAP_STACK: [u8; TRAP_STACK_SIZE_PAGES * PAGE_SIZE] = const { [0; TRAP_STACK_SIZE_PAGES * PAGE_SIZE] };
}

// `default_trap_entry` writes 64 contiguous 8-byte slots — gp regs at
// `sp[0..32]`, fp regs at `sp[32..64]` — and unconditionally bumps sp by
// 0x210. Pin the struct's size and field offsets so the asm offsets and
// the Rust layout cannot drift apart.
const _: () = {
    assert!(core::mem::offset_of!(kunwind::Registers, gp) == 0x000);
    #[cfg(target_feature = "d")]
    {
        assert!(core::mem::offset_of!(kunwind::Registers, fp) == 0x100);
        assert!(core::mem::size_of::<kunwind::Registers>() == 0x200);
    }
    #[cfg(not(target_feature = "d"))]
    assert!(core::mem::size_of::<kunwind::Registers>() == 0x100);
    assert!(
        core::mem::size_of::<kunwind::Registers>() == core::mem::size_of::<kunwind::Registers>()
    );
};

pub fn init() {
    let trap_stack_top = trap_stack_top();
    tracing::trace!("setting sscratch to {:#x}", trap_stack_top);
    sscratch::set(trap_stack_top);

    tracing::trace!("setting trap vec to {:#x}", default_trap_entry as usize);
    // Safety: register access
    unsafe { stvec::write(default_trap_entry as usize, stvec::Mode::Direct) };
}

/// Top of this CPU's trap stack. `sscratch` must hold this value whenever
/// the CPU is outside the trap handler so the next trap's `csrrw sp, sscratch, sp`
/// lands the new trap frame on the trap stack.
fn trap_stack_top() -> usize {
    // Safety: TRAP_STACK is a valid CPU-local static of the queried size.
    unsafe {
        TRAP_STACK
            .as_ptr()
            .byte_add(TRAP_STACK_SIZE_PAGES * PAGE_SIZE) as usize
    }
}

// #[naked]
// unsafe extern "C" fn trap_vec() {
//     // When in vectored mode
//     // exceptions i.e. sync traps => BASE
//     // interrupts i.e. async traps => BASE + 4 * CAUSE
//     //
//     // We can use this to direct some traps that don't need
//     // expensive SBI call handling to cheaper handlers (like timers)
//     // Safety: inline assembly
//     unsafe {
//         naked_asm! {
//             ".align 2",
//             ".option push",
//             ".option norvc",
//             "j {default}", // exception
//             "j {default}", // supervisor software interrupt
//             "j {default}", // reserved
//             "j {default}", // reserved
//             "j {default}", // reserved
//             "j {default}", // supervisor timer interrupt
//             "j {default}", // reserved
//             "j {default}", // reserved
//             "j {default}", // reserved
//             "j {default}", // supervisor external interrupt
//             ".option pop",
//             default = sym default_trap_entry,
//         }
//     }
// }

#[unsafe(naked)]
unsafe extern "C" fn default_trap_entry() {
    naked_asm! {
        // FIXME this is a workaround for bug in rustc/llvm
        //  https://github.com/rust-lang/rust/issues/80608#issuecomment-1094267279
        ".attribute arch, \"rv64gc\"",
        ".align 4",
        ".cfi_startproc",

        // Set the CFI rule for the return address to always return zero
        // This is always the first frame on stack, there is nowhere to return to
        ".cfi_register ra, zero",

        "csrrw sp, sscratch, sp", // sp points to the kunwind::Registers

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

// https://github.com/emb-riscv/specs-markdown/blob/develop/exceptions-and-interrupts.md
// Note: The C-unwind here is important, we want the stable C ABI so we can call this function from
// assembly, but we also want to be able to unwind past it into the trampoline above (so stack traces
// are fully accurate)
extern "C-unwind" fn default_trap_handler(
    frame: &mut kunwind::Registers,
    _a1: usize,
    _a2: usize,
    _a3: usize,
    _a4: usize,
    _a5: usize,
    _a6: usize,
    _a7: usize,
) {
    let cause = scause::read().cause();

    let epc = sepc::read();
    let tval = stval::read();
    tracing::trace!(
        "{cause:?} {:?};epc={epc:#x};tval={tval:#x}",
        sstatus::read()
    );
    let epc = VirtualAddress::new(epc);
    let tval = VirtualAddress::new(tval);
    let fp = VirtualAddress::new(frame.gp[8]); // fp is x8

    if IN_TRAP.replace(true) {
        handle_recursive_fault(frame, epc);
    }

    // Each arm clears `IN_TRAP` itself before returning. Arms that tail-call
    // into `handle_kernel_exception` do not, since that function never
    // returns — it restores the invariants itself before unwinding.
    match cause {
        Trap::Interrupt(Interrupt::SupervisorSoft) => {
            // Just a nop, software interrupts are only used as wakeup calls
            // TODO this should be an specialized routine in the trap vector
            //
            // Safety: register access
            unsafe { sip::clear_ssoft() };
            IN_TRAP.set(false);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            let (expired, maybe_next_deadline) = global().timer.try_turn().unwrap_or((0, None));
            if expired > 0 {
                global().executor.wake_one();
            }
            global().timer.schedule_wakeup(maybe_next_deadline);
            IN_TRAP.set(false);
        }
        Trap::Interrupt(Interrupt::SupervisorExternal) => {
            irq::trigger_irq(&mut *cpu_local().arch.cpu.interrupt_controller());
            global().executor.wake_one();
            IN_TRAP.set(false);
        }
        Trap::Exception(
            Exception::LoadPageFault | Exception::StorePageFault | Exception::InstructionPageFault,
        ) => {
            // first attempt the page fault handler, can it recover us from this by fixing up mappings?
            if crate::mem::handle_page_fault(cause, tval).is_break() {
                IN_TRAP.set(false);
                return;
            }

            // if not attempt the wasm fault handler, is the current trap caused by a user program?
            // if so can it kill the program?
            if let ControlFlow::Break(saved) =
                crate::wasm::trap_handler::handle_wasm_exception(epc, fp, tval)
            {
                redirect_sret_to(frame, saved);
                IN_TRAP.set(false);
                return;
            }

            handle_kernel_exception(cause, frame, epc, tval)
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            if let ControlFlow::Break(saved) =
                crate::wasm::trap_handler::handle_wasm_exception(epc, fp, tval)
            {
                redirect_sret_to(frame, saved);
                IN_TRAP.set(false);
                return;
            }

            handle_kernel_exception(cause, frame, epc, tval)
        }
        _ => handle_kernel_exception(cause, frame, epc, tval),
    }
}

/// Redirect the upcoming `sret` to the register context in `saved`.
///
/// The trap epilogue restores GPRs and FPRs from `frame`, swaps `sp` with
/// `sscratch`, and `sret`s using `sepc`. Stamping `saved` into `frame` and
/// pointing `sscratch`/`sepc` at the saved `SP`/`RA` therefore makes the
/// return land in `saved`'s execution context instead of the one that
/// originally trapped.
fn redirect_sret_to(frame: &mut kunwind::Registers, saved: kunwind::Registers) {
    *frame = saved;
    sscratch::set(frame[RiscV::SP]);
    sepc::set(frame[RiscV::RA]);
}

fn handle_kernel_exception(
    cause: Trap,
    frame: &kunwind::Registers,
    epc: VirtualAddress,
    tval: VirtualAddress,
) -> ! {
    // let's go ahead and begin unwinding the stack that caused the fault
    // Note: we use unwinding here to give kernel code the chance to catch this and recover from it.
    // If the unwinding reaches the root `catch_unwind` in `main.rs` this will tear down the entire
    // system causing all CPUs to shut down.

    tracing::error!("KERNEL TRAP {cause:?} epc={epc};tval={tval}");

    let mut regs = kunwind::Registers {
        gp: frame.gp,
        fp: frame.fp,
    };
    regs.gp[2] = sscratch::read();

    match Backtrace::<32>::from_registers(regs.clone(), epc) {
        Ok(bt) => tracing::error!("{bt}"),
        Err(e) => tracing::error!("backtrace unavailable: {e}; epc={epc}"),
    }

    // FIXME it would be great to get rid of the allocation here :/
    let payload = Box::new(cause);

    // Unwinding runs on the kernel stack and never returns through the trap
    // epilogue, so restore the per-CPU trap invariants by hand before
    // leaving: `sscratch` must hold the trap-stack-top (next trap's
    // `csrrw sp, sscratch, sp` depends on it), and `IN_TRAP` must be clear
    // (next trap must not be classified as recursive).
    sscratch::set(trap_stack_top());
    IN_TRAP.set(false);

    // Safety: `regs` was captured on trap entry.
    unsafe { kpanic_unwind::begin_unwind(payload, regs, epc.add(1).get()) };
}

fn handle_recursive_fault(frame: &kunwind::Registers, epc: VirtualAddress) -> ! {
    let mut regs = kunwind::Registers {
        gp: frame.gp,
        fp: frame.fp,
    };
    regs.gp[2] = sscratch::read();

    tracing::error!("RECURSIVE TRAP epc={epc}");

    // `epc` may land in code without DWARF unwind info (asm trampolines,
    // SBI shims, etc.). Log the error and continue instead of unwrapping;
    // panicking here would only feed back through this same path.
    match Backtrace::<32>::from_registers(regs.clone(), epc) {
        Ok(bt) => tracing::error!("{bt}"),
        Err(e) => tracing::error!("backtrace unavailable: {e}; epc={epc}"),
    }

    // FIXME it would be great to get rid of the allocation here :/
    let payload = Box::new("recursive fault in trap handler");

    // Safety: `regs` was captured on trap entry.
    unsafe {
        kpanic_unwind::begin_unwind(payload, regs, epc.get());
    }
}

#[cfg(test)]
mod tests {
    use gimli::RiscV;

    use super::IN_TRAP;
    use crate::tests::wast::WastContext;

    /// `gimli::RiscV` aliases must land in the GPR/FPR slot assigned by the
    /// RISC-V psABI. `s2..s11` live at `x18..x27` (not `x10..x19`) and
    /// `fs2..fs11` at `f18..f27` — code that patches a [`kunwind::Registers`] via
    /// `frame[RiscV::S2] = ...` relies on this mapping holding.
    #[ktest::test]
    async fn registers_index_matches_riscv_abi() {
        let mut r = kunwind::Registers::default();

        r[RiscV::ZERO] = 0xA0;
        assert_eq!(r.gp[0], 0xA0);
        r[RiscV::RA] = 0xA1;
        assert_eq!(r.gp[1], 0xA1);
        r[RiscV::SP] = 0xA2;
        assert_eq!(r.gp[2], 0xA2);
        r[RiscV::GP] = 0xA3;
        assert_eq!(r.gp[3], 0xA3);
        r[RiscV::TP] = 0xA4;
        assert_eq!(r.gp[4], 0xA4);

        r[RiscV::A0] = 0xB0;
        assert_eq!(r.gp[10], 0xB0);
        r[RiscV::A7] = 0xB7;
        assert_eq!(r.gp[17], 0xB7);

        r[RiscV::S0] = 0xC0;
        assert_eq!(r.gp[8], 0xC0);
        r[RiscV::S1] = 0xC1;
        assert_eq!(r.gp[9], 0xC1);
        r[RiscV::S2] = 0xD2;
        assert_eq!(r.gp[18], 0xD2);
        r[RiscV::S11] = 0xDB;
        assert_eq!(r.gp[27], 0xDB);

        #[cfg(target_feature = "d")]
        {
            r[RiscV::FS0] = 0xE0;
            assert_eq!(r.fp[8], 0xE0);
            r[RiscV::FS1] = 0xE1;
            assert_eq!(r.fp[9], 0xE1);
            r[RiscV::FS2] = 0xE2;
            assert_eq!(r.fp[18], 0xE2);
            r[RiscV::FS11] = 0xEB;
            assert_eq!(r.fp[27], 0xEB);
        }
    }

    /// A wasm trap must leave `IN_TRAP` cleared on the CPU that handled it,
    /// so the next trap on that CPU is not misclassified as recursive.
    ///
    /// `WastContext::run` and the underlying `catch_traps` are synchronous,
    /// so the trap fires on the same CPU executing this future and the
    /// cpu-local `IN_TRAP` we read here corresponds to that CPU.
    #[ktest::test]
    async fn in_trap_cleared_after_wasm_trap() {
        let mut ctx = WastContext::new_default().unwrap();
        ctx.run(
            "in_trap_cleared_after_wasm_trap",
            include_str!("../../../../tests/trap.wast"),
        )
        .await
        .unwrap();

        assert!(!IN_TRAP.get(), "IN_TRAP not cleared after wasm trap");
    }

    /// Several back-to-back wasm traps on the same CPU must each leave the
    /// trap-handler invariants intact.
    #[ktest::test]
    async fn repeated_wasm_traps() {
        let mut ctx = WastContext::new_default().unwrap();
        for i in 0..5 {
            ctx.run(
                "repeated_wasm_traps",
                include_str!("../../../../tests/trap.wast"),
            )
            .await
            .unwrap_or_else(|e| panic!("iteration {i} failed: {e}"));
            assert!(!IN_TRAP.get(), "iteration {i}: IN_TRAP stuck");
        }
    }

    /// A synchronous in-kernel trap that the page-fault recovery path can't
    /// fix up must reach the kernel-exception path, propagate up the kernel
    /// stack as a Rust panic, and land in `catch_unwind` cleanly. After the
    /// catch, the per-CPU trap invariants must be restored.
    #[ktest::test]
    async fn in_kernel_trap_unwinds_via_panic() {
        let result = kpanic_unwind::catch_unwind(|| {
            // Safety: deliberate null deref to provoke a LoadPageFault.
            let _ = unsafe { core::ptr::null::<usize>().read_volatile() };
            unreachable!("expected kernel trap to unwind past this");
        });
        let _payload = result.expect_err("expected kernel trap to unwind via panic");

        assert!(!IN_TRAP.get(), "IN_TRAP not cleared after kernel trap");
    }

    /// After a wasm trap, the next trap on the same CPU must be handled
    /// normally — not misclassified as recursive (`IN_TRAP` must be clear)
    /// and not landed on the wrong stack (`sscratch` must point at the trap
    /// stack). Provokes a self-IPI immediately after a wasm trap to verify
    /// both invariants in a single end-to-end check.
    #[ktest::test]
    async fn trap_after_wasm_trap_is_handled_normally() {
        let mut ctx = WastContext::new_default().unwrap();
        ctx.run(
            "trap_after_wasm_trap_is_handled_normally",
            include_str!("../../../../tests/trap.wast"),
        )
        .await
        .unwrap();
        assert!(!IN_TRAP.get(), "IN_TRAP stuck after wasm trap");

        // `sie.SSIE` and `sstatus.SIE` are both set at boot, so writing
        // `sip.SSIP` delivers a SupervisorSoft trap at the next instruction
        // boundary; the handler clears `sip.SSIP` and returns here.
        //
        // Safety: register access; we want the side effect.
        unsafe { riscv::sip::set_ssoft() };

        assert!(!IN_TRAP.get(), "IN_TRAP not cleared after SupervisorSoft");
    }
}
