// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

pub mod device;
mod setjmp_longjmp;
mod trap_handler;
mod utils;
mod vm;

use crate::arch::riscv64::device::cpu::with_cpu_info;
use crate::device_tree::DeviceTree;
use crate::time;
use crate::vm::VirtualAddress;
use bitflags::bitflags;
use core::arch::asm;
use core::cell::Cell;
use core::time::Duration;
use dtb_parser::Strings;
use fallible_iterator::FallibleIterator;
use riscv::sstatus::FS;
use riscv::{interrupt, scounteren, sie, sstatus};
pub use setjmp_longjmp::{call_with_setjmp, longjmp, JmpBuf};
use thread_local::thread_local;
pub use vm::{
    invalidate_range, is_kernel_address, AddressSpace, CANONICAL_ADDRESS_MASK, DEFAULT_ASID,
    KERNEL_ASPACE_BASE, PAGE_SHIFT, PAGE_SIZE, USER_ASPACE_BASE,
};

/// Global RISC-V specific initialization.
#[cold]
pub fn init() {
    let supported = riscv::sbi::supported_extensions().unwrap();
    log::trace!("Supported SBI extensions: {supported:?}");

    vm::init();
}

/// Per-hart and RISC-V specific initialization.
#[cold]
pub fn per_hart_init(devtree: &DeviceTree) -> crate::Result<()> {
    device::cpu::init(devtree)?;

    Ok(())
}

/// Early per-hart and RISC-V specific initialization.
///
/// This function will be called before global initialization is done, notably this function
/// cannot call logging functions, cannot allocate memory, cannot access hart-local state and should
/// not panic as the panic handler is not initialized yet.
#[cold]
pub fn per_hart_init_early() {
    // Safety: register access
    unsafe {
        // enable counters
        scounteren::set_cy();
        scounteren::set_tm();
        scounteren::set_ir();

        // Set the FPU state to initial
        sstatus::set_fs(FS::Initial);
    }
}

/// Late per-hart and RISC-V specific initialization.
///
/// This function will be called after all global initialization is done.
#[cold]
pub fn per_hart_init_late(devtree: &DeviceTree) -> crate::Result<()> {
    device::cpu::init(devtree)?;

    // Safety: register access
    unsafe {
        // Initialize the trap handler
        trap_handler::init();

        // Enable interrupts
        interrupt::enable();

        // Enable supervisor timer and external interrupts
        sie::set_ssie();
        sie::set_stie();
        sie::set_seie();
    }

    Ok(())
}

bitflags! {
    #[derive(Debug, Default, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct RiscvExtensions: u64 {
        const I = 1 << 0;
        const M = 1 << 1;
        const A = 1 << 2;
        const F = 1 << 3;
        const D = 1 << 4;
        const C = 1 << 5;
        const H = 1 << 6;
        const ZIC64B = 1 << 7;
        const ZICBOM = 1 << 8;
        const ZICBOP = 1 << 9;
        const ZICBOZ = 1 << 10;
        const ZICCAMOA = 1 << 11;
        const ZICCIF = 1 << 12;
        const ZICCLSM = 1 << 13;
        const ZICCRSE = 1 << 14;
        const ZICNTR = 1 << 15;
        const ZICSR = 1 << 16;
        const ZIFENCEI = 1 << 17;
        const ZIHINTNTL = 1 << 18;
        const ZIHINTPAUSE = 1 << 19;
        const ZIHPM = 1 << 20;
        const ZMMUL = 1 << 21;
        const ZA64RS = 1 << 22;
        const ZAAMO = 1 << 23;
        const ZALRSC = 1 << 24;
        const ZAWRS = 1 << 25;
        const ZFA = 1 << 26;
        const ZCA = 1 << 27;
        const ZCD = 1 << 28;
        const ZBA = 1 << 29;
        const ZBB = 1 << 30;
        const ZBC = 1 << 31;
        const ZBS = 1 << 32;
        const SSCCPTR = 1 << 33;
        const SSCOUNTERENW = 1 << 34;
        const SSTC = 1 << 35;
        const SSTVALA = 1 << 36;
        const SSTVECD = 1 << 37;
        const SVADU = 1 << 38;
        const SVVPTC = 1 << 39;
    }
}

pub fn parse_riscv_extensions(mut strs: Strings) -> Result<RiscvExtensions, dtb_parser::Error> {
    let mut out = RiscvExtensions::empty();

    while let Some(str) = strs.next()? {
        out |= match str {
            "i" => RiscvExtensions::I,
            "m" => RiscvExtensions::M,
            "a" => RiscvExtensions::A,
            "f" => RiscvExtensions::F,
            "d" => RiscvExtensions::D,
            "c" => RiscvExtensions::C,
            "h" => RiscvExtensions::H,
            "zic64b" => RiscvExtensions::ZIC64B,
            "zicbom" => RiscvExtensions::ZICBOM,
            "zicbop" => RiscvExtensions::ZICBOP,
            "zicboz" => RiscvExtensions::ZICBOZ,
            "ziccamoa" => RiscvExtensions::ZICCAMOA,
            "ziccif" => RiscvExtensions::ZICCIF,
            "zicclsm" => RiscvExtensions::ZICCLSM,
            "ziccrse" => RiscvExtensions::ZICCRSE,
            "zicntr" => RiscvExtensions::ZICNTR,
            "zicsr" => RiscvExtensions::ZICSR,
            "zifencei" => RiscvExtensions::ZIFENCEI,
            "zihintntl" => RiscvExtensions::ZIHINTNTL,
            "zihintpause" => RiscvExtensions::ZIHINTPAUSE,
            "zihpm" => RiscvExtensions::ZIHPM,
            "zmmul" => RiscvExtensions::ZMMUL,
            "za64rs" => RiscvExtensions::ZA64RS,
            "zaamo" => RiscvExtensions::ZAAMO,
            "zalrsc" => RiscvExtensions::ZALRSC,
            "zawrs" => RiscvExtensions::ZAWRS,
            "zfa" => RiscvExtensions::ZFA,
            "zca" => RiscvExtensions::ZCA,
            "zcd" => RiscvExtensions::ZCD,
            "zba" => RiscvExtensions::ZBA,
            "zbb" => RiscvExtensions::ZBB,
            "zbc" => RiscvExtensions::ZBC,
            "zbs" => RiscvExtensions::ZBS,
            "ssccptr" => RiscvExtensions::SSCCPTR,
            "sscounterenw" => RiscvExtensions::SSCOUNTERENW,
            "sstc" => RiscvExtensions::SSTC,
            "sstvala" => RiscvExtensions::SSTVALA,
            "sstvecd" => RiscvExtensions::SSTVECD,
            "svadu" => RiscvExtensions::SVADU,
            "svvptc" => RiscvExtensions::SVVPTC,
            _ => {
                log::error!("unknown RISCV extension {str}");
                // TODO better error type
                return Err(dtb_parser::Error::InvalidToken(0));
            }
        }
    }

    Ok(out)
}

/// Set the thread pointer on the calling hart to the given address.
pub fn set_thread_ptr(addr: VirtualAddress) {
    // Safety: inline assembly
    unsafe {
        asm!("mv tp, {addr}", addr = in(reg) addr.get());
    }
}

#[inline]
/// Returns the current stack pointer.
pub fn get_stack_pointer() -> usize {
    let stack_pointer: usize;
    // Safety: inline assembly
    unsafe {
        asm!(
            "mv {}, sp",
            out(reg) stack_pointer,
            options(nostack,nomem),
        );
    }
    stack_pointer
}

/// Retrieves the next older program counter and stack pointer from the current frame pointer.
pub unsafe fn get_next_older_pc_from_fp(fp: usize) -> usize {
    // Safety: caller has to ensure fp is valid
    unsafe { *(fp as *mut usize).offset(1) }
}

// The current frame pointer points to the next older frame pointer.
pub const NEXT_OLDER_FP_FROM_FP_OFFSET: usize = 0;

/// Asserts that the frame pointer is sufficiently aligned for the platform.
pub fn assert_fp_is_aligned(fp: usize) {
    assert_eq!(fp % 16, 0, "stack should always be aligned to 16");
}

pub fn mb() {
    // Safety: inline assembly
    unsafe {
        asm!("fence iorw,iorw");
    }
}
pub fn wmb() {
    // Safety: inline assembly
    unsafe {
        asm!("fence ow,ow");
    }
}
pub fn rmb() {
    // Safety: inline assembly
    unsafe {
        asm!("fence ir,ir");
    }
}

#[inline]
pub unsafe fn with_user_memory_access<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Allow supervisor access to user memory
    // Safety: register access
    unsafe {
        sstatus::set_sum();
    }

    let r = f();

    // Disable supervisor access to user memory
    // Safety: register access
    unsafe {
        sstatus::clear_sum();
    }

    r
}

/// Suspend the calling hart indefinitely.
///
/// # Safety
///
/// The caller must ensure it is safe to suspend the hart.
pub unsafe fn hart_park() {
    // Safety: inline assembly
    unsafe { asm!("wfi") }
}

/// Send an interrupt to a parked hart waking it up.
///
/// # Safety
///
/// The caller must ensure it is safe to send an interrupt to the target hart, which it generally should
/// be as the trap handler for software interrupts should be non-disruptive to already running harts,
/// but the caller should still exercise caution.
pub unsafe fn hart_unpark(hartid: usize) {
    riscv::sbi::ipi::send_ipi(1 << hartid, 0).unwrap();
}

thread_local! {
    static IN_TIMEOUT: Cell<bool> = Cell::new(false);
}

/// Suspend the calling hart for at least `duration`.
///
/// # Safety
///
/// The caller must ensure the duration does not overflow when converted into ticks, and that it
/// is safe to suspend the hart.
pub unsafe fn hart_park_timeout(duration: Duration) {
    // Safety: ensured by caller
    unsafe {
        IN_TIMEOUT.set(true);

        let timebase_freq = with_cpu_info(|info| info.timebase_frequency);
        riscv::sbi::time::set_timer(
            riscv::time::read64() + time::duration_to_ticks_unchecked(duration, timebase_freq),
        )
        .unwrap();

        if IN_TIMEOUT.get() {
            hart_park();
        }
    }
}
