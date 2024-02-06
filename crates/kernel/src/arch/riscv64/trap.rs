use crate::arch;
use crate::arch::backtrace::Context;
use core::arch::asm;
use core::marker::PhantomPinned;
use riscv::register::scause::{Exception, Trap};
use riscv::register::stvec::TrapMode;
use riscv::register::{scause, sepc, sie, sstatus, stval, stvec};

pub fn init() -> crate::Result<()> {
    unsafe {
        stvec::write(trap_vec as _, TrapMode::Vectored);
        sie::set_stimer();
        sstatus::set_sie();
    }

    Ok(())
}

/// This struct keeps the harts state during a trap so we can restore it later.
///
/// Currently, we only save the `t` and `a` registers as well as the `ra` register.
// TODO we probably should save all general purpose registers & floating points regs if kernel code is allowed to use them
#[repr(C, align(16))]
#[derive(Debug)]
pub struct TrapFrame {
    pub ra: usize,
    pub sp: usize,
    pub t: [usize; 7],
    pub a: [usize; 8],
    pub s: [usize; 12],

    pub _pinned: PhantomPinned,
}

#[naked]
pub unsafe extern "C" fn trap_vec() {
    // When in vectored mode
    // exceptions i.e. sync traps => BASE
    // interrupts i.e. async traps => BASE + 4 * CAUSE
    //
    // We can use this to direct some traps that don't need
    // expensive SBI call handling to cheaper handlers (like timers)
    asm! (
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
    options(noreturn)
    )
}

#[cfg(target_pointer_width = "32")]
macro_rules! save {
    ($reg:ident => $ptr:ident[$pos:expr]) => {
        concat!(
            "sw ",
            stringify!($reg),
            ", 4*",
            $pos,
            '(',
            stringify!($ptr),
            ')'
        )
    };
}

#[cfg(target_pointer_width = "32")]
macro_rules! load {
    ($ptr:ident[$pos:expr] => $reg:ident) => {
        concat!(
            "lw ",
            stringify!($reg),
            ", 4*",
            $pos,
            '(',
            stringify!($ptr),
            ')'
        )
    };
}

#[cfg(target_pointer_width = "64")]
macro_rules! save {
    ($reg:ident => $ptr:ident[$pos:expr]) => {
        concat!(
            "sd ",
            stringify!($reg),
            ", 8*",
            $pos,
            '(',
            stringify!($ptr),
            ')'
        )
    };
}

#[cfg(target_pointer_width = "64")]
macro_rules! load {
    ($ptr:ident[$pos:expr] => $reg:ident) => {
        concat!(
            "ld ",
            stringify!($reg),
            ", 8*",
            $pos,
            '(',
            stringify!($ptr),
            ')'
        )
    };
}

#[naked]
unsafe extern "C" fn default_trap_entry() {
    asm! {
        ".align 2",

        "csrrw t6, sscratch, t6", // t6 points to the TrapFrame

        save!(ra => t6[0]),
        save!(sp => t6[1]),
        save!(t0 => t6[2]),
        save!(t1 => t6[3]),
        save!(t2 => t6[4]),
        save!(t3 => t6[5]),
        save!(t4 => t6[6]),
        save!(t5 => t6[7]),
        // skip t6 because it's saved in sscratch
        save!(a0 => t6[9]),
        save!(a1 => t6[10]),
        save!(a2 => t6[11]),
        save!(a3 => t6[12]),
        save!(a4 => t6[13]),
        save!(a5 => t6[14]),
        save!(a6 => t6[15]),
        save!(a7 => t6[16]),

        save!(s0 => t6[17]),
        save!(s1 => t6[18]),
        save!(s2 => t6[19]),
        save!(s3 => t6[20]),
        save!(s4 => t6[21]),
        save!(s5 => t6[22]),
        save!(s6 => t6[23]),
        save!(s7 => t6[24]),
        save!(s8 => t6[25]),
        save!(s9 => t6[26]),
        save!(s10 => t6[27]),
        save!(s11 => t6[28]),

        "mv a0, t6",

        "call {trap_handler}",

        "mv t6, a0",

        load!(t6[0] => ra),
        load!(t6[1] => sp),
        load!(t6[2] => t0),
        load!(t6[3] => t1),
        load!(t6[4] => t2),
        load!(t6[5] => t3),
        load!(t6[6] => t4),
        load!(t6[7] => t5),
        // skip t6 because it's saved in sscratch
        load!(t6[9] => a0),
        load!(t6[10] => a1),
        load!(t6[11] => a2),
        load!(t6[12] => a3),
        load!(t6[13] => a4),
        load!(t6[14] => a5),
        load!(t6[15] => a6),
        load!(t6[16] => a7),

        "csrrw t6, sscratch, t6",
        "sret",

        trap_handler = sym default_trap_handler,
        options(noreturn)
    }
}

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
    let frame = unsafe { &*raw_frame };
    let cause = scause::read().cause();

    match cause {
        Trap::Exception(Exception::LoadPageFault) => {
            let epc = sepc::read();
            let tval = stval::read();

            let ctx = Context::from_raw(frame.ra, frame.sp, frame.s);

            log::error!("KERNEL LOAD PAGE FAULT: epc {epc:#x?} tval {tval:#x?}");
            let mut count = 0;
            crate::backtrace::trace_with_context(ctx, |frame| {
                count += 1;
                log::debug!("{:<2}- {:#x?}", count, frame.symbol_address());
            });

            arch::halt();
        }
        Trap::Exception(Exception::StorePageFault) => {
            let epc = sepc::read();
            let tval = stval::read();

            let ctx = Context::from_raw(frame.ra, frame.sp, frame.s);

            log::error!("KERNEL STORE PAGE FAULT: epc {epc:#x?} tval {tval:#x?}");
            let mut count = 0;
            crate::backtrace::trace_with_context(ctx, |frame| {
                count += 1;
                log::debug!("{:<2}- {:#x?}", count, frame.symbol_address());
            });

            arch::halt();
        }
        _ => {
            panic!("trap_handler cause {cause:?}, a1 {a1:#x} a2 {a2:#x} a3 {a3:#x} a4 {a4:#x} a5 {a5:#x} a6 {a6:#x} a7 {a7:#x}");
        }
    }

    // if matches!(cause, Trap::Interrupt(Interrupt::SupervisorTimer)) {
    //     log::debug!("timer event");
    //
    //     unsafe {
    //         sie::clear_stimer();
    //     }
    // } else {
    //     panic!("unknown trap")
    // }

    raw_frame
}
