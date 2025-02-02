// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! sip register

use crate::register::{clear_csr, read_csr_as, set_clear_csr_field, set_csr};

/// sip register
#[derive(Clone, Copy, Debug)]
pub struct Sip {
    bits: usize,
}

impl Sip {
    /// Returns the contents of the register as raw bits
    #[inline]
    pub fn bits(&self) -> usize {
        self.bits
    }

    /// Supervisor Software Interrupt Pending
    #[inline]
    pub fn ssoft(&self) -> bool {
        self.bits & (1 << 1) != 0
    }

    /// Supervisor Timer Interrupt Pending
    #[inline]
    pub fn stimer(&self) -> bool {
        self.bits & (1 << 5) != 0
    }

    /// Supervisor External Interrupt Pending
    #[inline]
    pub fn sext(&self) -> bool {
        self.bits & (1 << 9) != 0
    }
}

read_csr_as!(Sip, 0x144);
set_csr!(0x144);
clear_csr!(0x144);

set_clear_csr_field!(
    /// Supervisor Software Interrupt Pending
    , set_ssoft, clear_ssoft, 1 << 1_i32);
