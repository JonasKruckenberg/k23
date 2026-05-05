// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    Uefi(uefi::Error),
    Elf(object::Error),
    Alloc(mem_core::AllocError),

    /// Wraps any error from the in-tree `fdt` crate.
    Fdt(fdt::Error),
    /// A `PT_LOAD` segment carries a `p_flags` combination the loader does not
    /// support (anything other than `R`, `R|W`, or `R|X`).
    InvalidSegmentFlags(u32),
    /// Could not determine the boot CPU id from any source (rv64 only — the
    /// `RISCV_EFI_BOOT_PROTOCOL` lookup failed and `/chosen/boot-hartid`
    /// was absent).
    NoBootCpuId,
    NoRngSeed,
    /// The SMBIOS config table did not start with a valid `_SM3_` 3.0 entry
    /// point anchor, so it could not be staged.
    BadSmbios,
}

impl From<uefi::Error> for Error {
    fn from(err: uefi::Error) -> Self {
        Self::Uefi(err)
    }
}

impl From<fdt::Error> for Error {
    fn from(err: fdt::Error) -> Self {
        Self::Fdt(err)
    }
}

impl From<object::Error> for Error {
    fn from(err: object::Error) -> Self {
        Self::Elf(err)
    }
}

impl From<mem_core::AllocError> for Error {
    fn from(err: mem_core::AllocError) -> Self {
        Self::Alloc(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Uefi(err) => write!(f, "UEFI error {err}"),
            Error::Elf(err) => write!(f, "Failed to parse kernel elf: {err}"),
            Error::Fdt(err) => write!(f, "FDT parse error: {err}"),
            Error::InvalidSegmentFlags(flags) => write!(
                f,
                "kernel ELF has a PT_LOAD segment with unsupported flags {flags:#x}"
            ),
            Error::Alloc(_) => write!(
                f,
                "Failed to allocate physical frames for kernel address space"
            ),
            Error::NoBootCpuId => write!(f, "could not determine boot CPU id from firmware"),
            Error::NoRngSeed => write!(f, "could not seed RNG from firmware"),
            Error::BadSmbios => write!(f, "SMBIOS config table has no valid _SM3_ entry point"),
        }
    }
}
