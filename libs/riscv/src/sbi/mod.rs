//! SBI (RISC-V Supervisor Binary Interface).
//!
//! SBI is a standard interface for interacting with the "supervisor execution environment" on RISC-V.
//! This environment provided by the previous stage bootloader (most commonly OpenSBI) is responsible for
//! implementing the SBI functions.
//!
//! You can think of the "supervisor execution environment" as a minimal operating system,
//! running in M-mode that provides services to the operating system running in S-mode.

pub mod base;
pub mod dbcn;
mod error;
pub mod hsm;
pub mod rfence;
pub mod time;

pub use error::Error;

const EID_BASE: usize = 0x10;
const EID_HSM: usize = 0x0048_534D;
const EID_TIME: usize = 0x5449_4D45;
const EID_RFENCE: usize = 0x5246_4E43;
const EID_DBCN: usize = 0x4442_434E;

type Result<T> = core::result::Result<T, Error>;

macro_rules! sbi_call {
    (ext: $ext:expr, func: $func:expr) => {{
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                let error: isize;
                let value: usize;

                unsafe {
                    ::core::arch::asm!(
                        "ecall",
                        in("a6") $func, in("a7") $ext,
                        lateout("a0") error, lateout("a1") value,
                    )
                };

                if error == 0 {
                    Ok(value)
                } else {
                    match error {
                        -1 => Err($crate::sbi::Error::Failed),
                        -2 => Err($crate::sbi::Error::NotSupported),
                        -3 => Err($crate::sbi::Error::InvalidParam),
                        -4 => Err($crate::sbi::Error::Denied),
                        -5 => Err($crate::sbi::Error::InvalidAddress),
                        -6 => Err($crate::sbi::Error::AlreadyAvailable),
                        -7 => Err($crate::sbi::Error::AlreadyStarted),
                        -8 => Err($crate::sbi::Error::AlreadyStopped),
                        -9 => Err($crate::sbi::Error::NoShmem),
                        code => Err($crate::sbi::Error::Other(code)),
                    }
                }
            }  else {
                #[inline(always)]
                fn unimplemented() -> super::Result<usize> {
                    unimplemented!()
                }
                unimplemented()
            }
        }
    }};
    (ext: $ext:expr, func: $func:expr, $($reg:tt: $args:expr),*) => {{
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                let error: isize;
                let value: usize;

                unsafe {
                    ::core::arch::asm!(
                        "ecall",
                        $(in($reg) $args),*,
                        in("a6") $func, in("a7") $ext,
                        lateout("a0") error, lateout("a1") value,
                    )
                };

                if error == 0 {
                    Ok(value)
                } else {
                    match error {
                        -1 => Err($crate::sbi::Error::Failed),
                        -2 => Err($crate::sbi::Error::NotSupported),
                        -3 => Err($crate::sbi::Error::InvalidParam),
                        -4 => Err($crate::sbi::Error::Denied),
                        -5 => Err($crate::sbi::Error::InvalidAddress),
                        -6 => Err($crate::sbi::Error::AlreadyAvailable),
                        -7 => Err($crate::sbi::Error::AlreadyStarted),
                        -8 => Err($crate::sbi::Error::AlreadyStopped),
                        -9 => Err($crate::sbi::Error::NoShmem),
                        code => Err($crate::sbi::Error::Other(code)),
                    }
                }
            } else {
                $(let _ = $args);*;

                #[inline(always)]
                fn unimplemented() -> super::Result<usize> {
                    unimplemented!()
                }
                unimplemented()
            }
        }
    }}
}

pub(crate) use sbi_call;
