use super::csr_base_and_read;
use core::fmt;
use core::fmt::Formatter;
use crate::arch::csr_write;
use crate::arch::stvec::Mode;

csr_base_and_read!(Sepc, "sepc");
csr_write!("sepc");

pub unsafe fn write(sepc: usize) {
    _write(sepc);
}

impl fmt::Debug for Sepc {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Sepc").field(&self.bits).finish()
    }
}
