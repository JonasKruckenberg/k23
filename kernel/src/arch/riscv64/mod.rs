mod setjmp_longjmp;
pub mod trap_handler;

use riscv::sstatus::FS;
use riscv::{interrupt, sie, sstatus};

pub use setjmp_longjmp::{longjmp, setjmp, JumpBuf};

pub fn finish_processor_init() {
    unsafe {
        interrupt::enable();
        sie::set_stie();
        sstatus::set_fs(FS::Initial);
    }
}
