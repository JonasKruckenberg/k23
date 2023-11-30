use core::arch::asm;

pub mod backtrace;
pub mod interrupt;
pub mod paging;
mod start;
pub mod tls;
pub mod trap;

pub const PAGE_SIZE: usize = 4096;

pub const STACK_SIZE_PAGES: usize = 25;

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}
