#![no_std]
#![feature(error_in_core)]

mod error;
pub mod hsm;
pub mod rfence;
pub mod time;

use core::arch::asm;
pub use error::Error;

type Result<T> = core::result::Result<T, Error>;

const EID_HSM: usize = 0x48534D;
const EID_TIME: usize = 0x54494D45;
const EID_RFENCE: usize = 0x52464E43;

#[inline]
fn sbi_call(
    extension: usize,
    function: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> Result<usize> {
    let (error, value);
    unsafe {
        asm!(
            "ecall",
            in("a0") arg0, in("a1") arg1, in("a2") arg2, in("a3") arg3,
            in("a6") function, in("a7") extension,
            lateout("a0") error, lateout("a1") value,
        )
    };

    if error == 0 {
        Ok(value)
    } else {
        match error {
            -1 => Err(Error::Failed),
            -2 => Err(Error::NotSupported),
            -3 => Err(Error::InvalidParam),
            -4 => Err(Error::Denied),
            -5 => Err(Error::InvalidAddress),
            -6 => Err(Error::AlreadyAvailable),
            -7 => Err(Error::AlreadyStarted),
            -8 => Err(Error::AlreadyStopped),
            -9 => Err(Error::NoShmem),
            code => Err(Error::Other(code)),
        }
    }
}
