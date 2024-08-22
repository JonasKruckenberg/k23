//! Supervisor Interrupt Enable Register

use super::{csr_base_and_read, csr_clear, csr_write};
use core::fmt;
use core::fmt::Formatter;

csr_base_and_read!(Sie, "sie");
csr_write!("sie");
csr_clear!("sie");

pub unsafe fn set_ssie() {
    _write(1 << 1);
}

pub unsafe fn set_stie() {
    _write(1 << 5);
}

pub unsafe fn set_seie() {
    _write(1 << 9);
}

pub unsafe fn clear_ssie() {
    _clear(1 << 1);
}

pub unsafe fn clear_stie() {
    _clear(1 << 5);
}

pub unsafe fn clear_seie() {
    _clear(1 << 9);
}

impl Sie {
    #[must_use]
    pub fn ssie(&self) -> bool {
        self.bits & (1 << 1) != 0
    }

    #[must_use]
    pub fn stie(&self) -> bool {
        self.bits & (1 << 5) != 0
    }

    #[must_use]
    pub fn seie(&self) -> bool {
        self.bits & (1 << 9) != 0
    }
}

impl fmt::Debug for Sie {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sie")
            .field("ssie", &self.ssie())
            .field("stie", &self.stie())
            .field("seie", &self.seie())
            .finish()
    }
}
