//! RISC-V architecture support crate.
#![no_std]

mod error;
pub mod hio;
pub mod interrupt;
pub mod register;
pub mod sbi;
pub mod semihosting;

use core::arch::asm;

pub use error::Error;
pub use register::*;
pub type Result<T> = core::result::Result<T, Error>;

/// Terminates the current execution with the specified exit code.
///
/// This will use the semihosting interface, if available, to exit the program. Otherwise, it will
/// enter a wfi loop.
pub fn exit(code: i32) -> ! {
    semihosting::exit(code);

    // fall back to a wfi loop if exiting using semihosting failed
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

/// Terminates the current execution in an abnormal fashion.
pub fn abort() -> ! {
    exit(1);
}
