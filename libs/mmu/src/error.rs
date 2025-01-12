// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    /// The system was not able to allocate memory needed for the operation.
    NoMemory,
    /// Attempted to operate on mismatched address space.
    AddressSpaceMismatch { expected: usize, found: usize },
    /// The operation attempted to increase permissions.
    PermissionIncrease,
    /// Errors returned by SBI calls
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    SBI(riscv::sbi::Error),
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
impl From<riscv::sbi::Error> for Error {
    fn from(err: riscv::sbi::Error) -> Self {
        Error::SBI(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NoMemory => write!(f, "The system was not able to allocate memory needed for the operation"),
            Error::AddressSpaceMismatch { expected, found } => write!(f, "Attempted to operate on mismatched address space. Expected {expected} but found {found}."),
            Error::PermissionIncrease => write!(f, "The operation attempted to increase permissions"),
            #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
            Error::SBI(err) => write!(f, "SBI call failed: {err}"),
        }
    }
}

impl core::error::Error for Error {}

#[macro_export]
#[allow(edition_2024_expr_fragment_specifier)]
macro_rules! ensure {
    ($cond:expr, $error:expr) => {
        if !$cond {
            return Err($error);
        }
    };
}
