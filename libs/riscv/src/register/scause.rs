// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Supervisor Cause Register

use super::{read_csr_as, write_csr};
use core::fmt;
use core::fmt::Formatter;
pub use trap::{Trap, Interrupt, Exception};

/// scause register
#[derive(Clone, Copy)]
pub struct Scause {
    bits: usize,
}

read_csr_as!(Scause, 0x142);
write_csr!(0x142);

pub unsafe fn set(trap: Trap) {
    match trap {
        Trap::Interrupt(interrupt) => unsafe {
            _write(1 << (usize::BITS as usize - 1) | interrupt as usize);
        },
        Trap::Exception(exception) => unsafe { _write(exception as usize) },
    }
}

impl Scause {
    /// Returns the code field
    #[inline]
    #[must_use]
    pub fn code(&self) -> usize {
        self.bits & !(1 << (usize::BITS as usize - 1))
    }

    /// Is trap cause an interrupt.
    #[inline]
    #[must_use]
    pub fn is_interrupt(&self) -> bool {
        self.bits & (1 << (usize::BITS as usize - 1)) != 0
    }

    /// Is trap cause an exception.
    #[inline]
    #[must_use]
    pub fn is_exception(&self) -> bool {
        !self.is_interrupt()
    }

    /// Returns the cause of the trap.
    ///
    /// # Panics
    ///
    /// Panics if the cause is unknown or invalid.
    #[inline]
    #[must_use]
    pub fn cause(&self) -> Trap {
        if self.is_interrupt() {
            Trap::Interrupt(Interrupt::try_from(self.code()).expect("unknown interrupt"))
        } else {
            Trap::Exception(Exception::try_from(self.code()).expect("unknown exception"))
        }
    }
}


impl fmt::Debug for Scause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Scause").field(&self.cause()).finish()
    }
}
