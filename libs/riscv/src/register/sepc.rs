use super::csr_base_and_read;
use core::fmt;
use core::fmt::Formatter;

csr_base_and_read!(Sepc, "stvec");

impl fmt::Debug for Sepc {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Sepc").field(&self.bits).finish()
    }
}
