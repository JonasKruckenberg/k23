mod setjmp_longjmp;
pub mod trap_handler;
pub mod vm;

use core::arch::asm;
use riscv::sstatus::FS;
use riscv::{interrupt, sie, sstatus};

pub use setjmp_longjmp::{call_with_setjmp, longjmp, setjmp, JmpBuf, JmpBufStruct};

pub fn finish_hart_init() {
    unsafe {
        interrupt::enable();
        sie::set_stie();
        sstatus::set_fs(FS::Initial);
    }
}

pub fn wait_for_interrupt() {
    unsafe { asm!("wfi") }
}
