// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

/// Errors returned by SBI calls
#[derive(Debug)]
pub enum Error {
    Failed,
    NotSupported,
    InvalidParam,
    Denied,
    InvalidAddress,
    AlreadyAvailable,
    AlreadyStarted,
    AlreadyStopped,
    NoShmem,
    Other(isize),
    IntConversion(core::num::TryFromIntError),
}

impl From<core::num::TryFromIntError> for Error {
    fn from(err: core::num::TryFromIntError) -> Self {
        Error::IntConversion(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Failed => f.write_str("Failed"),
            Error::NotSupported => f.write_str("Not supported"),
            Error::InvalidParam => f.write_str("Invalid parameter(s)"),
            Error::Denied => f.write_str("Denied or not allowed"),
            Error::InvalidAddress => f.write_str("Invalid address(s)"),
            Error::AlreadyAvailable => f.write_str("Already available"),
            Error::AlreadyStarted => f.write_str("Already started"),
            Error::AlreadyStopped => f.write_str("Already stopped"),
            Error::NoShmem => f.write_str("No shared memory available"),
            Error::Other(code) => f.write_fmt(format_args!("Other error: {code}")),
            Error::IntConversion(err) => f.write_fmt(format_args!("Failed to convert int {err}")),
        }
    }
}

impl core::error::Error for Error {}
