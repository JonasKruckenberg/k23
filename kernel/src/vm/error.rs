// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter, Write};

#[derive(Debug)]
pub enum Error {
    SizeTooLarge,
    MisalignedStart,
    MisalignedEnd,
    AlignmentTooLarge,
    InvalidVmoOffset,
    InvalidPermissions,
    PermissionIncrease,
    AlreadyMapped,
    NotMapped,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::SizeTooLarge => f.write_str("address range size must be less than or equal to the maximum address space size"),
            Error::MisalignedStart => f.write_str("address range start must be at least page aligned"),
            Error::MisalignedEnd => f.write_str("address range end must be at least page aligned"),
            Error::AlignmentTooLarge => f.write_str("alignment must less than or equal to the maximum support alignment"),
            Error::InvalidVmoOffset => f.write_str("offset must be valid for the given VMO"),
            Error::InvalidPermissions => f.write_str("requested permissions must be R^X"),
            Error::PermissionIncrease => f.write_str("protect can only be used to reduce permissions, never increase them"),
            Error::AlreadyMapped => f.write_str("requested address range is already mapped"),
            Error::NotMapped => f.write_str("requested address range is not mapped"),
        }
    }
}

impl core::error::Error for Error {}