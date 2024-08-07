pub mod hio;
pub mod register;
pub mod sbi;
pub(crate) mod semihosting;
pub mod unwinding;

use core::arch::asm;
pub use register::*;

pub fn abort_internal(code: i32) -> ! {
    semihosting::exit(code);

    // fall back to a wfi loop if exiting using semihosting failed
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}
