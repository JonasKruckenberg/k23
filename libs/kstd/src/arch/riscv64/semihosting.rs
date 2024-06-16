#[macro_export]
macro_rules! syscall {
    ($nr:path) => {
        $crate::arch::riscv64::semihosting::syscall_inner($nr, 0)
    };
    ($nr:path, $a1:expr) => {
        $crate::arch::riscv64::semihosting::syscall_inner(
            $nr,
            &[$a1 as usize] as *const usize as usize,
        )
    };
    ($nr:path, $a1:expr, $a2:expr) => {
        $crate::arch::riscv64::semihosting::syscall_inner(
            $nr,
            &[$a1 as usize, $a2 as usize] as *const usize as usize,
        )
    };
    ($nr:path, $a1:expr, $a2:expr, $a3:expr) => {
        $crate::arch::riscv64::semihosting::syscall_inner(
            $nr,
            &[$a1 as usize, $a2 as usize, $a3 as usize] as *const usize as usize,
        )
    };
    ($nr:path, $a1:expr, $a2:expr, $a3:expr, $a4:expr) => {
        $crate::arch::riscv64::semihosting::syscall_inner(
            $nr,
            &[$a1 as usize, $a2 as usize, $a3 as usize, $a4 as usize] as *const usize as usize,
        )
    };
}

#[inline(always)]
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
