pub mod trap_handler;

use riscv::sstatus::FS;
use riscv::{interrupt, sie, sstatus};

pub fn finish_processor_init() {
    unsafe {
        interrupt::enable();
        sie::set_stie();
        sstatus::set_fs(FS::Initial);
    }
}
