// new filw
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! x86_64 error types

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    GenericError,
    // InvalidSelector,
    // InvalidAddress,
    // NotSupported,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::GenericError => write!(f, "x86_64 error"),
            // Error::InvalidSelector => write!(f, "invalid selector"),
            // Error::InvalidAddress => write!(f, "invalid address"),
            // Error::NotSupported => write!(f, "operation not supported"),
        }
    }
}

impl core::error::Error for Error {}