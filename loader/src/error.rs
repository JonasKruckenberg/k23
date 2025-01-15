// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    /// Failed to convert number
    TryFromInt(core::num::TryFromIntError),
    /// Failed to parse device tree blob
    Dtb(dtb_parser::Error),
    /// Failed to parse kernel elf
    Elf(&'static str),
    /// The system was not able to allocate memory needed for the operation.
    NoMemory,
}

impl From<core::num::TryFromIntError> for Error {
    fn from(err: core::num::TryFromIntError) -> Self {
        Error::TryFromInt(err)
    }
}

impl From<dtb_parser::Error> for Error {
    fn from(err: dtb_parser::Error) -> Self {
        Error::Dtb(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NoMemory => write!(
                f,
                "The system was not able to allocate memory needed for the operation"
            ),
            Error::TryFromInt(_) => write!(f, "Failed to convert number"),
            Error::Dtb(err) => write!(f, "Failed to parse device tree blob: {err}"),
            Error::Elf(err) => write!(f, "Failed to parse kernel elf: {err}"),
        }
    }
}
