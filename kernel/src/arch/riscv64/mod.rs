pub mod trap_handler;
mod setjmp_longjmp;

use riscv::sstatus::FS;
use riscv::{interrupt, sie, sstatus};

pub use setjmp_longjmp::{JumpBuf, setjmp, longjmp};

pub fn finish_processor_init() {
    unsafe {
        interrupt::enable();
        sie::set_stie();
        sstatus::set_fs(FS::Initial);
    }
}
