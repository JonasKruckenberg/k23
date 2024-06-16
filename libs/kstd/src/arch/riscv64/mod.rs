pub mod hio;
pub mod register;
pub mod sbi;
pub(crate) mod semihosting;

use core::arch::asm;
pub use register::*;

pub fn abort_internal() -> ! {
    semihosting::exit(1);

    // fall back to a wfi loop if exiting using semihosting failed
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}
