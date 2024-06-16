pub mod base;
pub mod dbcn;
pub mod error;
pub mod hsm;
pub mod rfence;
pub mod time;

pub use error::Error;

const EID_BASE: usize = 0x10;
const EID_HSM: usize = 0x48534D;
const EID_TIME: usize = 0x54494D45;
const EID_RFENCE: usize = 0x52464E43;
const EID_DBCN: usize = 0x4442434E;

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
                        -1 => Err($crate::arch::riscv64::sbi::Error::Failed),
                        -2 => Err($crate::arch::riscv64::sbi::Error::NotSupported),
                        -3 => Err($crate::arch::riscv64::sbi::Error::InvalidParam),
                        -4 => Err($crate::arch::riscv64::sbi::Error::Denied),
                        -5 => Err($crate::arch::riscv64::sbi::Error::InvalidAddress),
                        -6 => Err($crate::arch::riscv64::sbi::Error::AlreadyAvailable),
                        -7 => Err($crate::arch::riscv64::sbi::Error::AlreadyStarted),
                        -8 => Err($crate::arch::riscv64::sbi::Error::AlreadyStopped),
                        -9 => Err($crate::arch::riscv64::sbi::Error::NoShmem),
                        code => Err($crate::arch::riscv64::sbi::Error::Other(code)),
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
                        -1 => Err($crate::arch::riscv64::sbi::Error::Failed),
                        -2 => Err($crate::arch::riscv64::sbi::Error::NotSupported),
                        -3 => Err($crate::arch::riscv64::sbi::Error::InvalidParam),
                        -4 => Err($crate::arch::riscv64::sbi::Error::Denied),
                        -5 => Err($crate::arch::riscv64::sbi::Error::InvalidAddress),
                        -6 => Err($crate::arch::riscv64::sbi::Error::AlreadyAvailable),
                        -7 => Err($crate::arch::riscv64::sbi::Error::AlreadyStarted),
                        -8 => Err($crate::arch::riscv64::sbi::Error::AlreadyStopped),
                        -9 => Err($crate::arch::riscv64::sbi::Error::NoShmem),
                        code => Err($crate::arch::riscv64::sbi::Error::Other(code)),
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
