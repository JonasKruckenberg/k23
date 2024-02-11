use core::arch::asm;

pub mod backtrace;
pub mod interrupt;
pub mod paging;
mod start;

pub type VMM = kmem::Riscv64Sv39;

pub const STACK_SIZE_PAGES: usize = 25;

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}
