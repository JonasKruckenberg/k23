use core::arch::asm;
use kmem::{Arch, VirtualAddress, GIB, MIB};

pub mod backtrace;
pub mod interrupt;
pub mod paging;
mod start;

pub type MemoryMode = kmem::Riscv64Sv39;

pub const STACK_SIZE_PAGES: usize = 25;

pub const PAGE_SIZE: usize = MemoryMode::PAGE_SIZE;

pub const HEAP_SIZE: usize = 64 * MIB;

pub const HEAP_BASE: VirtualAddress =
    unsafe { VirtualAddress::new(usize::MAX & !MemoryMode::ADDR_OFFSET_MASK).sub(2 * GIB) };

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}
