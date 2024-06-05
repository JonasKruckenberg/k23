mod register;

use core::arch::asm;
pub use register::*;

pub mod sbi;
pub mod semihosting;

pub fn abort() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}
