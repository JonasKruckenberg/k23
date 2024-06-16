pub mod hio;
pub mod register;
pub mod sbi;
pub(crate) mod semihosting;

use core::arch::asm;
pub use register::*;

pub fn abort_internal() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}
