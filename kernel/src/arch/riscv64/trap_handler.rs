use crate::TRAP_STACK_SIZE_PAGES;
use core::arch::{asm, naked_asm};
use mmu::arch::PAGE_SIZE;
use riscv::scause::{Exception, Interrupt, Trap};
use riscv::{scause, sepc, sstatus, stval, stvec};
use thread_local::thread_local;

thread_local! {
    static TRAP_STACK: [u8; TRAP_STACK_SIZE_PAGES * PAGE_SIZE] = const { [0; TRAP_STACK_SIZE_PAGES * PAGE_SIZE] };
}

pub fn init() {
    let trap_stack_top = unsafe {
        TRAP_STACK
            .as_ptr()
            .byte_add(TRAP_STACK_SIZE_PAGES * PAGE_SIZE) as *mut u8
    };

    log::debug!("setting sscratch to {:p}", trap_stack_top);
    unsafe {
        asm!(
            "csrrw x0, sscratch, {trap_frame}", // sscratch points to the trap frame
            trap_frame = in(reg) trap_stack_top
        );
    }

    log::debug!("setting trap vec to {:#x}", trap_vec as usize);
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

#[allow(clippy::too_many_lines)]
#[naked]
unsafe extern "C" fn default_trap_entry() {
    naked_asm! {
        ".align 2",

        "mv t0, sp", // save the correct stack pointer
        "csrrw sp, sscratch, sp", // t6 points to the TrapFrame
        "add sp, sp, -0x210",

        // save gp
        "
            sd x0, 0x00(sp)
            sd ra, 0x08(sp)
            sd t0, 0x10(sp)
            sd gp, 0x18(sp)
            sd tp, 0x20(sp)
            sd s0, 0x40(sp)
            sd s1, 0x48(sp)
            sd s2, 0x90(sp)
            sd s3, 0x98(sp)
            sd s4, 0xA0(sp)
            sd s5, 0xA8(sp)
            sd s6, 0xB0(sp)
            sd s7, 0xB8(sp)
            sd s8, 0xC0(sp)
            sd s9, 0xC8(sp)
            sd s10, 0xD0(sp)
            sd s11, 0xD8(sp)
            ",

        // save fp
        "
            fsd fs0, 0x140(sp)
            fsd fs1, 0x148(sp)
            fsd fs2, 0x190(sp)
            fsd fs3, 0x198(sp)
            fsd fs4, 0x1A0(sp)
            fsd fs5, 0x1A8(sp)
            fsd fs6, 0x1B0(sp)
            fsd fs7, 0x1B8(sp)
            fsd fs8, 0x1C0(sp)
            fsd fs9, 0x1C8(sp)
            fsd fs10, 0x1D0(sp)
            fsd fs11, 0x1D8(sp)
        ",

        "mv a0, sp",

        "call {trap_handler}",

        "mv sp, a0",

        // restore gp
        "ld ra, 0x08(a0)",
        // skip sp since it is saved in sscratch
        "ld gp, 0x18(a0)
            ld tp, 0x20(a0)
            ld t0, 0x28(a0)
            ld t1, 0x30(a0)
            ld t2, 0x38(a0)
            ld s0, 0x40(a0)
            ld s1, 0x48(a0)
            ld a1, 0x58(a0)
            ld a2, 0x60(a0)
            ld a3, 0x68(a0)
            ld a4, 0x70(a0)
            ld a5, 0x78(a0)
            ld a6, 0x80(a0)
            ld a7, 0x88(a0)
            ld s2, 0x90(a0)
            ld s3, 0x98(a0)
            ld s4, 0xA0(a0)
            ld s5, 0xA8(a0)
            ld s6, 0xB0(a0)
            ld s7, 0xB8(a0)
            ld s8, 0xC0(a0)
            ld s9, 0xC8(a0)
            ld s10, 0xD0(a0)
            ld s11, 0xD8(a0)
            ld t3, 0xE0(a0)
            ld t4, 0xE8(a0)
            ld t5, 0xF0(a0)
            ld t6, 0xF8(a0)
            ",

        // restore fp
        "
            fld ft0, 0x100(a0)
            fld ft1, 0x108(a0)
            fld ft2, 0x110(a0)
            fld ft3, 0x118(a0)
            fld ft4, 0x120(a0)
            fld ft5, 0x128(a0)
            fld ft6, 0x130(a0)
            fld ft7, 0x138(a0)
            fld fs0, 0x140(a0)
            fld fs1, 0x148(a0)
            fld fa0, 0x150(a0)
            fld fa1, 0x158(a0)
            fld fa2, 0x160(a0)
            fld fa3, 0x168(a0)
            fld fa4, 0x170(a0)
            fld fa5, 0x178(a0)
            fld fa6, 0x180(a0)
            fld fa7, 0x188(a0)
            fld fs2, 0x190(a0)
            fld fs3, 0x198(a0)
            fld fs4, 0x1A0(a0)
            fld fs5, 0x1A8(a0)
            fld fs6, 0x1B0(a0)
            fld fs7, 0x1B8(a0)
            fld fs8, 0x1C0(a0)
            fld fs9, 0x1C8(a0)
            fld fs10, 0x1D0(a0)
            fld fs11, 0x1D8(a0)
            fld ft8, 0x1E0(a0)
            fld ft9, 0x1E8(a0)
            fld ft10, 0x1F0(a0)
            fld ft11, 0x1F8(a0)
        ",

        "add sp, sp, 0x210",
        "csrrw sp, sscratch, sp",
        "sret",

        trap_handler = sym default_trap_handler,
    }
}

/// A special trampoline function that the trap handler switches to, to initiate a kernel panic & backtrace
/// *after* switching back to the regular stack since printing a backtrace of the trap stack is seldom helpful.
extern "C-unwind" fn trap_panic_trampoline() {
    panic!("UNRECOVERABLE KERNEL TRAP");
}

// https://github.com/emb-riscv/specs-markdown/blob/develop/exceptions-and-interrupts.md
#[allow(clippy::too_many_arguments)]
fn default_trap_handler(
    raw_frame: *mut TrapFrame,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
    a7: usize,
) -> *mut TrapFrame {
    let cause = scause::read().cause();

    log::trace!("{:?}", sstatus::read());
    log::trace!("trap_handler cause {cause:?}, a1 {a1:#x} a2 {a2:#x} a3 {a3:#x} a4 {a4:#x} a5 {a5:#x} a6 {a6:#x} a7 {a7:#x}");

    match cause {
        Trap::Exception(Exception::LoadPageFault) => {
            let epc = sepc::read();
            let tval = stval::read();

            log::error!("KERNEL LOAD PAGE FAULT: epc {epc:#x?} tval {tval:#x?}");
            sepc::set(trap_panic_trampoline as usize)
        }
        Trap::Exception(Exception::StorePageFault) => {
            let epc = sepc::read();
            let tval = stval::read();

            log::error!("KERNEL STORE PAGE FAULT: epc {epc:#x?} tval {tval:#x?}");
            sepc::set(trap_panic_trampoline as usize)
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            // just clear the timer interrupt when it happens for now, this is required to make the
            // tests in the `time` module work
            riscv::sbi::time::set_timer(u64::MAX).unwrap();
        }
        _ => {
            sepc::set(trap_panic_trampoline as usize)
            // panic!("trap_handler cause {cause:?}, a1 {a1:#x} a2 {a2:#x} a3 {a3:#x} a4 {a4:#x} a5 {a5:#x} a6 {a6:#x} a7 {a7:#x}");
        }
    }

    raw_frame
}
