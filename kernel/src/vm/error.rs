// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::VirtualAddress;
use core::fmt::{Display, Formatter};

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
    NoMemory,
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    CacheInvalidationFailed(riscv::sbi::Error),
    /// Attempted to operate on mismatched address space.
    AddressSpaceMismatch {
        expected: u16,
        found: u16,
    },
    /// Errors returned by SBI calls
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    Sbi(riscv::sbi::Error),
    KernelFaultInUserSpace(VirtualAddress),
    UserFaultInKernelSpace(VirtualAddress),
}

impl From<crate::vm::frame_alloc::AllocError> for Error {
    fn from(_: crate::vm::frame_alloc::AllocError) -> Self {
        Self::NoMemory
    }
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
impl From<riscv::sbi::Error> for Error {
    fn from(err: riscv::sbi::Error) -> Self {
        Error::Sbi(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::SizeTooLarge => f.write_str(
                "address range size must be less than or equal to the maximum address space size",
            ),
            Error::MisalignedStart => {
                f.write_str("address range start must be at least page aligned")
            }
            Error::MisalignedEnd => f.write_str("address range end must be at least page aligned"),
            Error::AlignmentTooLarge => {
                f.write_str("alignment must less than or equal to the maximum support alignment")
            }
            Error::InvalidVmoOffset => f.write_str("offset must be valid for the given VMO"),
            Error::InvalidPermissions => f.write_str("requested permissions must be R^X"),
            Error::PermissionIncrease => {
                f.write_str("protect can only be used to reduce permissions, never increase them")
            }
            Error::AlreadyMapped => f.write_str("requested address range is already mapped"),
            Error::NotMapped => f.write_str("requested address range is not mapped"),
            Error::NoMemory => f.write_str("failed to allocate memory for page table entry"),
            #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
            Error::CacheInvalidationFailed(err) => f.write_fmt(format_args!(
                "failed to invalidate page table caches: {err}"
            )),
            Error::AddressSpaceMismatch { expected, found } => write!(
                f,
                "Attempted to operate on mismatched address space. Expected {expected} but found {found}."
            ),
            #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
            Error::Sbi(err) => write!(f, "SBI call failed: {err}"),
            Error::KernelFaultInUserSpace(addr) => write!(
                f,
                "non-user address fault in user address space addr={addr:?}"
            ),
            Error::UserFaultInKernelSpace(addr) => write!(
                f,
                "non-kernel address fault in kernel address space addr={addr:?}"
            ),
        }
    }
}

impl core::error::Error for Error {}
