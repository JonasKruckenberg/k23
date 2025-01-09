// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    InvalidMagic,
    InvalidVersion,
    InvalidToken(u32),
    InvalidNesting,
    UnexpectedEOF,
    TryFromSlice(core::array::TryFromSliceError),
    Utf8(core::str::Utf8Error),
    FromBytesUntilNulError(core::ffi::FromBytesUntilNulError),
    MissingParent,
    IntConvert(core::num::TryFromIntError),
}

impl From<core::array::TryFromSliceError> for Error {
    fn from(err: core::array::TryFromSliceError) -> Self {
        Error::TryFromSlice(err)
    }
}

impl From<core::str::Utf8Error> for Error {
    fn from(err: core::str::Utf8Error) -> Self {
        Error::Utf8(err)
    }
}

impl From<core::ffi::FromBytesUntilNulError> for Error {
    fn from(err: core::ffi::FromBytesUntilNulError) -> Self {
        Error::FromBytesUntilNulError(err)
    }
}

impl From<core::num::TryFromIntError> for Error {
    fn from(err: core::num::TryFromIntError) -> Self {
        Error::IntConvert(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::InvalidMagic => write!(f, "invalid magic number"),
            Error::InvalidVersion => write!(f, "invalid version"),
            Error::InvalidToken(t) => write!(f, "invalid token: {t}"),
            Error::InvalidNesting => write!(f, "invalid tree nesting"),
            Error::UnexpectedEOF => write!(f, "unexpected end of file"),
            Error::TryFromSlice(err) => write!(f, "failed to parse slice: {err}"),
            Error::Utf8(err) => write!(f, "failed to parse utf8: {err}"),
            Error::FromBytesUntilNulError(err) => write!(f, "failed to parse C-string: {err}"),
            Error::MissingParent => {
                write!(f, "DTB properties must be preceded by their parent node")
            }
            Error::IntConvert(err) => write!(f, "failed to convert integer: {err}"),
        }
    }
}

impl core::error::Error for Error {}
