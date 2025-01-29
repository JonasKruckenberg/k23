// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;

#[derive(Debug)]
pub enum Error {
    UnexpectedEndOfData,
    InvalidUtf8(core::str::Utf8Error),
    InvalidCStr(core::ffi::FromBytesUntilNulError),
    InvalidToken(crate::parser::BigEndianToken),
    UnexpectedToken(crate::parser::BigEndianToken),
    NumericConversion(core::num::TryFromIntError),
    TryFromSlice(core::array::TryFromSliceError),
    SliceTooSmall,
    BadMagic,
    InvalidPropertyValue,
    InalidCellSize,
}

impl From<core::str::Utf8Error> for Error {
    fn from(err: core::str::Utf8Error) -> Self {
        Error::InvalidUtf8(err)
    }
}
impl From<core::ffi::FromBytesUntilNulError> for Error {
    fn from(err: core::ffi::FromBytesUntilNulError) -> Self {
        Error::InvalidCStr(err)
    }
}
impl From<core::num::TryFromIntError> for Error {
    fn from(err: core::num::TryFromIntError) -> Self {
        Error::NumericConversion(err)
    }
}
impl From<core::array::TryFromSliceError> for Error {
    fn from(err: core::array::TryFromSliceError) -> Self {
        Error::TryFromSlice(err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnexpectedEndOfData => write!(f, "unexpected end of data"),
            Error::InvalidUtf8(err) => write!(f, "invalid utf8: {err}"),
            Error::InvalidCStr(err) => write!(f, "invalid C string: {err}"),
            Error::InvalidToken(t) => write!(f, "invalid token: {}", t.0.to_ne()),
            Error::UnexpectedToken(t) => write!(f, "unexpected token: {}", t.0.to_ne()),
            Error::NumericConversion(err) => write!(f, "numeric conversion failed: {err}"),
            Error::SliceTooSmall => write!(f, "slice too small"),
            Error::BadMagic => write!(f, "bad magic number"),
            Error::InvalidPropertyValue => write!(f, "invalid property value"),
            Error::InalidCellSize => write!(f, "invalid cell size"),
            Error::TryFromSlice(err) => write!(f, "failed to parse slice: {err}"),
        }
    }
}

impl core::error::Error for Error {}
