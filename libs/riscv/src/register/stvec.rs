//! Supervisor Trap Vector Base Address Register

use super::{csr_base_and_read, csr_write};
use core::fmt;
use core::fmt::Formatter;

csr_base_and_read!(Stvec, "stvec");
csr_write!("stvec");

pub unsafe fn write(base: usize, mode: Mode) {
    _write(base + mode as usize);
}

impl Stvec {
    /// # Panics
    ///
    /// Panics if the mode is invalid.
    #[must_use]
    pub fn mode(&self) -> Mode {
        let mode = self.bits & 0b11;
        match mode {
            0 => Mode::Direct,
            1 => Mode::Vectored,
            _ => panic!("unknown trap mode"),
        }
    }
    #[must_use]
    pub fn base(&self) -> usize {
        self.bits - (self.bits & 0b11)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Mode {
    /// All exceptions set `pc` to `BASE`.
    Direct = 0,
    /// Asynchronous interrupts set `pc` to `BASE+4×cause`.
    Vectored = 1,
}

impl fmt::Debug for Stvec {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stvec")
            .field("mode", &self.mode())
            .field("base", &self.base())
            .finish()
    }
}
