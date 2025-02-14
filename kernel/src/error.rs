// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::frame_alloc;
use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    /// Failed to parse device tree blob
    Fdt(fdt::Error),
    /// Errors returned by SBI calls
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    Sbi(riscv::sbi::Error),
    /// The caller did not have permission to perform the specified operation.
    AccessDenied,
    /// An argument is invalid.
    InvalidArgument,
    /// An object with the specified identifier or at the specified place already exists.
    ///
    /// Example: creating a mapping for an address range that is already mapped.
    AlreadyExists,
    /// The system was not able to allocate some resource needed for the operation.
    NoResources,
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    /// An unknown RISC-V extension was found.
    UnknownRiscvExtension,
    /// The given resource was not yet initialized
    Uninitialized,
}

impl From<fdt::Error> for Error {
    fn from(err: fdt::Error) -> Self {
        Self::Fdt(err)
    }
}

impl From<frame_alloc::AllocError> for Error {
    fn from(_value: frame_alloc::AllocError) -> Self {
        Self::NoResources
    }
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
impl From<riscv::sbi::Error> for Error {
    fn from(err: riscv::sbi::Error) -> Self {
        Error::Sbi(err)
    }
}

impl From<cpu_local::AccessError> for Error {
    fn from(_value: cpu_local::AccessError) -> Self {
        Error::Uninitialized
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Fdt(err) => write!(f, "Failed to parse flattened device tree: {err}"),
            #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
            Error::Sbi(err) => write!(f, "SBI call failed: {err}"),
            Error::AccessDenied => {
                write!(
                    f,
                    "The caller did not have permission to perform the specified operation"
                )
            }
            Error::InvalidArgument => write!(f, "An argument is invalid"),
            Error::AlreadyExists => write!(
                f,
                "An object with the specified identifier or at the specified place already exists",
            ),
            Error::NoResources => write!(
                f,
                "The system was not able to allocate some resource needed for the operation",
            ),
            #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
            Error::UnknownRiscvExtension => {
                write!(f, "An unknown RISC-V extension was found")
            }
            Error::Uninitialized => write!(f, "The resource was not yet initialized"),
        }
    }
}

impl core::error::Error for Error {}

#[macro_export]
macro_rules! ensure {
    ($cond:expr, $error:expr, $msg:expr) => {
        if !$cond {
            log::error!($msg);
            return Err($error);
        }
    };
    ($cond:expr, $error:expr) => {
        if !$cond {
            return Err($error);
        }
    };
}

#[macro_export]
macro_rules! bail {
    ($error:expr) => {
        return Err($error);
    };
    ($error:expr, $msg:expr) => {
        log::error!($msg);
        return Err($error);
    };
}
