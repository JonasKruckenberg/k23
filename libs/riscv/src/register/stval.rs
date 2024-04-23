use super::csr_base_and_read;
use core::fmt;
use core::fmt::Formatter;

csr_base_and_read!(Stval, "stvec");

impl fmt::Debug for Stval {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Stval").field(&self.bits).finish()
    }
}
