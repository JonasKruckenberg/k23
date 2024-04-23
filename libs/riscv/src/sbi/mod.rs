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
    }};
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
    }}
}

pub(crate) use sbi_call;
