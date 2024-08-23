#[macro_export]
#[doc(hidden)]
macro_rules! syscall {
    ($nr:path) => {
        $crate::semihosting::syscall_inner($nr, 0)
    };
    ($nr:path, $a1:expr) => {
        $crate::semihosting::syscall_inner($nr, &[$a1 as usize] as *const usize as usize)
    };
    ($nr:path, $a1:expr, $a2:expr) => {
        #[allow(clippy::ref_as_ptr)]
        $crate::semihosting::syscall_inner(
            $nr,
            &[$a1 as usize, $a2 as usize] as *const usize as usize,
        )
    };
    ($nr:path, $a1:expr, $a2:expr, $a3:expr) => {
        #[allow(clippy::ref_as_ptr)]
        $crate::semihosting::syscall_inner(
            $nr,
            &[$a1 as usize, $a2 as usize, $a3 as usize] as *const usize as usize,
        )
    };
    ($nr:path, $a1:expr, $a2:expr, $a3:expr, $a4:expr) => {
        $crate::semihosting::syscall_inner(
            $nr,
            &[$a1 as usize, $a2 as usize, $a3 as usize, $a4 as usize] as *const usize as usize,
        )
    };
}

#[inline(always)]
#[allow(clippy::used_underscore_binding)]
pub(crate) unsafe fn syscall_inner(_nr: usize, _arg: usize) -> usize {
    cfg_if::cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            use core::arch::asm;

            let mut nr = _nr;
            let arg = _arg;
            // The instructions below must always be uncompressed, otherwise
            // it will be treated as a regular break, hence the norvc option.
            //
            // See https://github.com/riscv/riscv-semihosting-spec for more details.
            asm!("
                .balign 16
                .option push
                .option norvc
                slli x0, x0, 0x1f
                ebreak
                srai x0, x0, 0x7
                .option pop
            ",
            inout("a0") nr,
            inout("a1") arg => _,
            options(nostack, preserves_flags),
            );
            nr
        } else {
            unimplemented!();
        }
    }
}

pub use syscall;

/// [SYS_EXIT (0x18)](https://github.com/ARM-software/abi-aa/blob/HEAD/semihosting/semihosting.rst#sys_exit-0x18)
pub const SYS_EXIT: usize = 0x18;

#[cfg(target_pointer_width = "32")]
/// [SYS_EXIT_EXTENDED (0x20)](https://github.com/ARM-software/abi-aa/blob/HEAD/semihosting/semihosting.rst#sys_exit_extended-0x20)
pub const SYS_EXIT_EXTENDED: usize = 0x20;

#[derive(Debug, Clone, Copy)]
#[repr(usize)]
#[non_exhaustive]
pub enum ExitReason {
    // Reason codes related to hardware exceptions:
    // AdpStoppedBranchThroughZero = 0x20000,
    // AdpStoppedUndefinedInstr = 0x20001,
    // AdpStoppedSoftwareInterrupt = 0x20002,
    // AdpStoppedPrefetchAbort = 0x20003,
    // AdpStoppedDataAbort = 0x20004,
    // AdpStoppedAddressException = 0x20005,
    // AdpStoppedIrq = 0x20006,
    // AdpStoppedFiq = 0x20007,

    // Reason codes related to software events:
    // AdpStoppedBreakPoint = 0x20020,
    // AdpStoppedWatchPoint = 0x20021,
    // AdpStoppedStepComplete = 0x20022,
    AdpStoppedRunTimeErrorUnknown = 0x20023,
    // AdpStoppedInternalError = 0x20024,
    // AdpStoppedUserInterruption = 0x20025,
    AdpStoppedApplicationExit = 0x20026,
    // AdpStoppedStackOverflow = 0x20027,
    // AdpStoppedDivisionByZero = 0x20028,
    // AdpStoppedOsspecific = 0x20029,
}

#[allow(clippy::cast_sign_loss)]
pub(crate) fn exit(code: i32) {
    // TODO: check sh_ext_exit_extended first
    sys_exit_extended(
        ExitReason::AdpStoppedApplicationExit,
        code as isize as usize,
    );
    // If SYS_EXIT_EXTENDED is not supported, above call doesn't exit program,
    // so try again with SYS_EXIT.
    let reason = match code {
        0 => ExitReason::AdpStoppedApplicationExit,
        _ => ExitReason::AdpStoppedRunTimeErrorUnknown,
    };
    sys_exit(reason);
}

/// [SYS_EXIT (0x18)](https://github.com/ARM-software/abi-aa/blob/HEAD/semihosting/semihosting.rst#sys_exit-0x18)
pub fn sys_exit(reason: ExitReason) {
    unsafe {
        #[cfg(target_pointer_width = "32")]
        syscall!(SYS_EXIT, reason as usize);
        #[cfg(target_pointer_width = "64")]
        syscall!(SYS_EXIT, reason as usize, 0);
    }
}

/// [SYS_EXIT_EXTENDED (0x20)](https://github.com/ARM-software/abi-aa/blob/HEAD/semihosting/semihosting.rst#sys_exit_extended-0x20)
pub fn sys_exit_extended(reason: ExitReason, subcode: usize) {
    unsafe {
        #[cfg(target_pointer_width = "32")]
        syscall!(SYS_EXIT_EXTENDED, reason as usize, subcode);
        // On 64-bit system, SYS_EXIT_EXTENDED call is identical to the behavior of the mandatory SYS_EXIT.
        #[cfg(target_pointer_width = "64")]
        syscall!(SYS_EXIT, reason as usize, subcode);
    }
}
