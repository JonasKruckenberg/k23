// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Supervisor Interrupt Enable Register

use super::{clear, read_csr_as, set};
use core::fmt;
use core::fmt::Formatter;

/// sie register
#[derive(Clone, Copy)]
pub struct Sie {
    bits: usize,
}

read_csr_as!(Sie, 0x104);
set!(0x104);
clear!(0x104);

pub unsafe fn set_ssie() {
    unsafe {
        _set(1 << 1);
    }
}

pub unsafe fn set_stie() {
    unsafe {
        _set(1 << 5);
    }
}

pub unsafe fn set_seie() {
    unsafe {
        _set(1 << 9);
    }
}

pub unsafe fn clear_ssie() {
    unsafe {
        _clear(1 << 1);
    }
}

pub unsafe fn clear_stie() {
    unsafe {
        _clear(1 << 5);
    }
}

pub unsafe fn clear_seie() {
    unsafe {
        _clear(1 << 9);
    }
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
