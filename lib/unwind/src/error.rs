// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    /// Gimli error
    Gimli(gimli::Error),
    /// Rust cannot catch foreign exceptions
    ForeignException,
    /// Reached the end of the stack without finding a landing pad
    EndOfStack,
    /// The personality function is not a Rust personality function
    DifferentPersonality,
    /// Missing section
    MissingSection(&'static str),
    /// Attempted to unwind through a `nounwind` function
    NoUnwind,
}

impl From<gimli::Error> for Error {
    fn from(err: gimli::Error) -> Self {
        Error::Gimli(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Gimli(err) => write!(f, "Gimli error: {err}"),
            Error::ForeignException => write!(f, "Rust cannot catch foreign exceptions"),
            Error::EndOfStack => write!(f, "End of stack"),
            Error::DifferentPersonality => write!(
                f,
                "The personality function is not a Rust personality function"
            ),
            Error::MissingSection(err) => write!(f, "Missing section: {err}"),
            Error::NoUnwind => write!(f, "Attempted to unwind through a `nounwind` function"),
        }
    }
}

impl core::error::Error for Error {}
