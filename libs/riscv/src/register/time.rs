//! Time Register

use super::csr_base_and_read;
use core::fmt;
use core::fmt::Formatter;

csr_base_and_read!(Time, "0xC01");

impl fmt::Debug for Time {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Time").field(&self.bits).finish()
    }
}
