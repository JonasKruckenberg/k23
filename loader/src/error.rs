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
    Fdt(k23_fdt::Error),
    /// Failed to parse kernel elf
    Elf(&'static str),
    /// The system was not able to allocate memory needed for the operation.
    NoMemory,
    /// Failed to start secondary hart
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    FailedToStartSecondaryHart(k23_riscv::sbi::Error),
    TryFromSlice(core::array::TryFromSliceError),
}
impl From<core::num::TryFromIntError> for Error {
    fn from(err: core::num::TryFromIntError) -> Self {
        Error::TryFromInt(err)
    }
}
impl From<k23_fdt::Error> for Error {
    fn from(err: k23_fdt::Error) -> Self {
        Error::Fdt(err)
    }
}
impl From<core::array::TryFromSliceError> for Error {
    fn from(err: core::array::TryFromSliceError) -> Self {
        Error::TryFromSlice(err)
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
            Error::Fdt(err) => write!(f, "Failed to parse device tree blob: {err}"),
            Error::Elf(err) => write!(f, "Failed to parse kernel elf: {err}"),
            #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
            Error::FailedToStartSecondaryHart(err) => {
                write!(f, "Failed to start secondary hart: {err}")
            }
            Error::TryFromSlice(err) => write!(f, "failed to parse slice: {err}"),
        }
    }
}
