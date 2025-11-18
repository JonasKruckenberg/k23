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
    Fdt(fdt::Error),
    /// Failed to parse kernel elf
    Elf(&'static str),
    /// The system was not able to allocate the physical memory needed for the operation.
    FrameAlloc(kmem_core::AllocError),
    /// The system was not able to allocate the virtual memory needed for the operation.
    PageAlloc(crate::page_alloc::AllocError),
    /// Failed to start secondary hart
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    FailedToStartSecondaryHart(riscv::sbi::Error),
    TryFromSlice(core::array::TryFromSliceError),
}
impl From<core::num::TryFromIntError> for Error {
    fn from(err: core::num::TryFromIntError) -> Self {
        Error::TryFromInt(err)
    }
}
impl From<fdt::Error> for Error {
    fn from(err: fdt::Error) -> Self {
        Error::Fdt(err)
    }
}
impl From<core::array::TryFromSliceError> for Error {
    fn from(err: core::array::TryFromSliceError) -> Self {
        Error::TryFromSlice(err)
    }
}
impl From<kmem_core::AllocError> for Error {
    fn from(err: kmem_core::AllocError) -> Self {
        Self::FrameAlloc(err)
    }
}
impl From<crate::page_alloc::AllocError> for Error {
    fn from(err: crate::page_alloc::AllocError) -> Self {
        Self::PageAlloc(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::FrameAlloc(err) => write!(
                f,
                "The system was not able to allocate the physical memory needed for the operation: {err:?}"
            ),
            Error::PageAlloc(err) => write!(
                f,
                "The system was not able to allocate the virtual memory needed for the operation: {err:?}"
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
