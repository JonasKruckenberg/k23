// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    InvalidFieldValue {
        field: &'static str,
        value: usize,
        bitmask: usize,
    },
    IndexOutOfBounds {
        index: usize,
        min: i32,
        max: i32,
    },
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::InvalidFieldValue { .. } => f.write_str("Failed"),
            Error::IndexOutOfBounds { .. } => f.write_str("Failed"),
        }
    }
}

impl core::error::Error for Error {}
