// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::register::{clear, read_csr_as, set};
use crate::{set_clear_csr, Error};

/// Scounteren register
#[derive(Clone, Copy)]
pub struct Scounteren {
    bits: usize,
}

read_csr_as!(Scounteren, 0x106);
set!(0x106);
clear!(0x106);

set_clear_csr!(
/// User cycle Enable
    , set_cy, clear_cy, 1 << 0);

set_clear_csr!(
/// User time Enable
    , set_tm, clear_tm, 1 << 1);

set_clear_csr!(
/// User instret Enable
    , set_ir, clear_ir, 1 << 2);

impl Scounteren {
    /// User "cycle\[h\]" Enable
    #[inline]
    pub fn cy(&self) -> bool {
        self.bits & (1 << 0) != 0
    }

    /// User "time\[h\]" Enable
    #[inline]
    pub fn tm(&self) -> bool {
        self.bits & (1 << 1) != 0
    }

    /// User "instret\[h]\" Enable
    #[inline]
    pub fn ir(&self) -> bool {
        self.bits & (1 << 2) != 0
    }

    /// User "hpm\[x\]" Enable (bits 3-31)
    #[inline]
    pub fn hpm(&self, index: usize) -> bool {
        assert!((3..32).contains(&index));
        self.bits & (1 << index) != 0
    }

    /// User "hpm\[x\]" Enable (bits 3-31)
    ///
    /// Attempts to read the "hpm\[x\]" value, and returns an error if the index is invalid.
    #[inline]
    pub fn try_hpm(&self, index: usize) -> crate::Result<bool> {
        if (3..32).contains(&index) {
            Ok(self.bits & (1 << index) != 0)
        } else {
            Err(Error::IndexOutOfBounds {
                index,
                min: 3,
                max: 31,
            })
        }
    }
}
