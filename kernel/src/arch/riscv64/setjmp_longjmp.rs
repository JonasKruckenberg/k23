use core::arch::asm;
use core::ptr;

/// A store for the register state used by `setjmp` and `longjmp`.
///
/// In essence this marks a "checkpoint" in the program's execution that can be returned to later.
#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct JumpBuf {
    pc: usize,
    s: [usize; 12],
    sp: usize,
    fs: [usize; 12],
}

impl JumpBuf {
    pub const fn new() -> Self {
        Self {
            pc: 0,
            sp: 0,
            s: [0; 12],
            fs: [0; 12],
        }
    }
}

/// Helper macro for constructing the inline assembly, used below.
macro_rules! define_op {
    ($ins:literal, $reg:ident, $ptr_width:literal, $pos:expr, $ptr:ident) => {
        concat!(
            $ins,
            " ",
            stringify!($reg),
            ", ",
            stringify!($ptr_width),
            "*",
            $pos,
            '(',
            stringify!($ptr),
            ')'
        )
    };
}

// helper macros for loading and storing registers used by setjmp and longjmp
cfg_if::cfg_if! {
    if #[cfg(target_pointer_width = "32")] {
        macro_rules! save_gp {
            ($reg:ident => $ptr:ident[$pos:expr]) => {
                define_op!("sw", $reg, 4, $pos, $ptr)
            }
        }
        macro_rules! load_gp {
            ($ptr:ident[$pos:expr] => $reg:ident) => {
                define_op!("lw", $reg, 4, $pos, $ptr)
            }
        }
        macro_rules! save_fp {
            ($reg:ident => $ptr:ident[$pos:expr]) => {
                define_op!("fsw", $reg, 4, $pos, $ptr)
            }
        }
        macro_rules! load_fp {
            ($ptr:ident[$pos:expr] => $reg:ident) => {
                define_op!("flw", $reg, 4, $pos, $ptr)
            }
        }
    } else if #[cfg(target_pointer_width = "64")] {
        macro_rules! load_gp {
            ($ptr:ident[$pos:expr] => $reg:ident) => {
                define_op!("ld", $reg, 8, $pos, $ptr)
            }
        }
        macro_rules! save_gp {
            ($reg:ident => $ptr:ident[$pos:expr]) => {
                define_op!("sd", $reg, 8, $pos, $ptr)
            }
        }
        macro_rules! load_fp {
            ($ptr:ident[$pos:expr] => $reg:ident) => {
                define_op!("fld", $reg, 8, $pos, $ptr)
            }
        }
        macro_rules! save_fp {
            ($reg:ident => $ptr:ident[$pos:expr]) => {
                define_op!("fsd", $reg, 8, $pos, $ptr)
            }
        }
    }
}

/// Saves various information about the calling environment (the stack pointer,
/// the instruction pointer and callee saved registers) and establishes a "checkpoint"
/// to which control flow can later be transferred.
///
/// This function pretty weird, it can return more than one time:
/// - The first time it returns, the return value is `0` indicating that the context has been saved.
/// - Subsequently, calls to `longjmp` that transfer control to the `*mut JumpBuf` used by this `setjmp`
///     will cause this function to return again, this time with the value passed to `longjmp`.
///
/// This implementation has been adapted from the [LLVM libc implementation (Apache License v2.0 with LLVM Exceptions)](https://github.com/llvm/llvm-project/blob/bbf2ad026eb0b399364a889799ef6b45878cd299/libc/src/setjmp/riscv/setjmp.cpp)
///
/// # Safety
///
/// Due to the weird multi-return nature of `setjmp` it is very easy to make mistakes, this
/// function be used with extreme care.
#[naked]
pub unsafe extern "C" fn setjmp(buf: *mut JumpBuf) -> isize {
    cfg_if::cfg_if! {
        if #[cfg(target_feature = "d")] {
            asm! {
                save_gp!(ra => a0[0]),
                save_gp!(s0 => a0[1]),
                save_gp!(s1 => a0[2]),
                save_gp!(s2 => a0[3]),
                save_gp!(s3 => a0[4]),
                save_gp!(s4 => a0[5]),
                save_gp!(s5 => a0[6]),
                save_gp!(s6 => a0[7]),
                save_gp!(s7 => a0[8]),
                save_gp!(s8 => a0[9]),
                save_gp!(s9 => a0[10]),
                save_gp!(s10 => a0[11]),
                save_gp!(s11 => a0[12]),
                save_gp!(sp => a0[13]),

                save_fp!(fs0 => a0[14]),
                save_fp!(fs1 => a0[15]),
                save_fp!(fs2 => a0[16]),
                save_fp!(fs3 => a0[17]),
                save_fp!(fs4 => a0[18]),
                save_fp!(fs5 => a0[19]),
                save_fp!(fs6 => a0[20]),
                save_fp!(fs7 => a0[21]),
                save_fp!(fs8 => a0[22]),
                save_fp!(fs9 => a0[23]),
                save_fp!(fs10 => a0[24]),
                save_fp!(fs11 => a0[25]),

                "mv a0, zero",
                "ret",
                options(noreturn)
            }
        } else {
            asm! {
                save_gp!(ra => a0[0]),
                save_gp!(s0 => a0[1]),
                save_gp!(s1 => a0[2]),
                save_gp!(s2 => a0[3]),
                save_gp!(s3 => a0[4]),
                save_gp!(s4 => a0[5]),
                save_gp!(s5 => a0[6]),
                save_gp!(s6 => a0[7]),
                save_gp!(s7 => a0[8]),
                save_gp!(s8 => a0[9]),
                save_gp!(s9 => a0[10]),
                save_gp!(s10 => a0[11]),
                save_gp!(s11 => a0[12]),
                save_gp!(sp => a0[13]),
                "mv a0, zero",
                "ret",
                options(noreturn)
            }
        }
    }
}

/// Performs a non-local jump to a context previously saved by `setjmp`.
///
/// This implementation has been adapted from the [LLVM libc implementation (Apache License v2.0 with LLVM Exceptions)](https://github.com/llvm/llvm-project/blob/1ae0dae368e4bbf2177603d5c310e794c4fd0bd8/libc/src/setjmp/riscv/longjmp.cpp)
///
/// # Safety
///
/// This will transfer control to instructions saved in the `*mut JumpBuf` parameter,
/// so extreme care must be taken to ensure that the `JumpBuf` is valid and has been initialized.
/// Additionally, the whole point of this function is to continue execution at a wildly different
/// address, so this might cause confusing and hard-to-debug behavior.
#[naked]
pub unsafe extern "C" fn longjmp(buf: *mut JumpBuf, val: isize) -> ! {
    cfg_if::cfg_if! {
        if #[cfg(target_feature = "d")] {
            asm! {
                load_gp!(a0[0] => ra),
                load_gp!(a0[1] => s0),
                load_gp!(a0[2] => s1),
                load_gp!(a0[3] => s2),
                load_gp!(a0[4] => s3),
                load_gp!(a0[5] => s4),
                load_gp!(a0[6] => s5),
                load_gp!(a0[7] => s6),
                load_gp!(a0[8] => s7),
                load_gp!(a0[9] => s8),
                load_gp!(a0[10] => s9),
                load_gp!(a0[11] => s10),
                load_gp!(a0[12] => s11),
                load_gp!(a0[13] => sp),

                load_fp!(a0[14] => fs0),
                load_fp!(a0[15] => fs1),
                load_fp!(a0[16] => fs2),
                load_fp!(a0[17] => fs3),
                load_fp!(a0[18] => fs4),
                load_fp!(a0[19] => fs5),
                load_fp!(a0[20] => fs6),
                load_fp!(a0[21] => fs7),
                load_fp!(a0[22] => fs8),
                load_fp!(a0[23] => fs9),
                load_fp!(a0[24] => fs10),
                load_fp!(a0[25] => fs11),

                "add a0, a1, zero",
                "ret",
                options(noreturn)
            }
        } else {
            asm! {
                load_gp!(a0[0] => ra),
                load_gp!(a0[1] => s0),
                load_gp!(a0[2] => s1),
                load_gp!(a0[3] => s2),
                load_gp!(a0[4] => s3),
                load_gp!(a0[5] => s4),
                load_gp!(a0[6] => s5),
                load_gp!(a0[7] => s6),
                load_gp!(a0[8] => s7),
                load_gp!(a0[9] => s8),
                load_gp!(a0[10] => s9),
                load_gp!(a0[11] => s10),
                load_gp!(a0[12] => s11),
                load_gp!(a0[13] => sp),

                "add a0, a1, zero",
                "ret",
                options(noreturn)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ptr;
    use core::ptr::addr_of_mut;

    #[ktest::test]
    fn setjmp_longjmp_simple() {
        unsafe {
            let mut c = 0;
            let mut buf = JumpBuf::new();

            let r = setjmp(ptr::from_mut(&mut buf));
            c += 1;
            if r == 0 {
                assert_eq!(c, 1);
                longjmp(ptr::from_mut(&mut buf), 1234567);
            }
            assert_eq!(c, 2);
            assert_eq!(r, 1234567);
        }
    }

    static mut BUFFER_A: JumpBuf = JumpBuf::new();
    static mut BUFFER_B: JumpBuf = JumpBuf::new();

    #[ktest::test]
    fn setjmp_longjmp_complex() {
        unsafe fn routine_a() {
            let r = setjmp(addr_of_mut!(BUFFER_A));
            if r == 0 {
                routine_b()
            }
            assert_eq!(r, 10001);

            let r = setjmp(addr_of_mut!(BUFFER_A));
            if r == 0 {
                longjmp(addr_of_mut!(BUFFER_B), 20001);
            }
            assert_eq!(r, 10002);

            let r = setjmp(addr_of_mut!(BUFFER_A));
            if r == 0 {
                longjmp(addr_of_mut!(BUFFER_B), 20002);
            }
            debug_assert!(r == 10003);
        }

        unsafe fn routine_b() {
            let r = setjmp(addr_of_mut!(BUFFER_B));
            if r == 0 {
                longjmp(addr_of_mut!(BUFFER_A), 10001);
            }
            assert_eq!(r, 20001);

            let r = setjmp(addr_of_mut!(BUFFER_B));
            if r == 0 {
                longjmp(addr_of_mut!(BUFFER_A), 10002);
            }
            assert_eq!(r, 20002);

            let r = setjmp(addr_of_mut!(BUFFER_B));
            if r == 0 {
                longjmp(addr_of_mut!(BUFFER_A), 10003);
            }
        }

        unsafe {
            routine_a();
        }
    }
}
