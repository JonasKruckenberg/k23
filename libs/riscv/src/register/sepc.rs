//! Supervisor Exception Program Counter Register

use super::csr_base_and_read;
use crate::csr_write;
use core::fmt;
use core::fmt::Formatter;

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
