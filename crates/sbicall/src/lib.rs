#![no_std]
#![feature(error_in_core)]

mod error;
pub mod hsm;
pub mod rfence;
pub mod time;

pub use error::Error;

type Result<T> = core::result::Result<T, Error>;

const EID_HSM: usize = 0x48534D;
const EID_TIME: usize = 0x54494D45;
const EID_RFENCE: usize = 0x52464E43;

macro_rules! sbi_call {
    (ext: $ext:expr, func: $func:expr, $($reg:tt: $args:expr),*) => {{
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
                -1 => Err($crate::Error::Failed),
                -2 => Err($crate::Error::NotSupported),
                -3 => Err($crate::Error::InvalidParam),
                -4 => Err($crate::Error::Denied),
                -5 => Err($crate::Error::InvalidAddress),
                -6 => Err($crate::Error::AlreadyAvailable),
                -7 => Err($crate::Error::AlreadyStarted),
                -8 => Err($crate::Error::AlreadyStopped),
                -9 => Err($crate::Error::NoShmem),
                code => Err($crate::Error::Other(code)),
            }
        }
    }}
}

pub(crate) use sbi_call;
