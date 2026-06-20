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
    Fdt(fdt::Error),
    Boot(loader_common::Error),
    /// The SMBIOS config table did not start with a valid `_SM3_` 3.0 entry
    /// point anchor, so it could not be staged.
    BadSmbios,
    NoRngSeed,
    /// Could not determine the boot HART id from any source.
    NoBootHartId,
    NoKernel,
}

impl From<uefi::Error> for Error {
    fn from(err: uefi::Error) -> Self {
        Self::Uefi(err)
    }
}

impl From<loader_common::Error> for Error {
    fn from(err: loader_common::Error) -> Self {
        Self::Boot(err)
    }
}

impl From<fdt::Error> for Error {
    fn from(err: fdt::Error) -> Self {
        Self::Fdt(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Uefi(err) => write!(f, "UEFI error {err}"),
            Error::Fdt(err) => write!(f, "FDT parse error: {err}"),
            Error::BadSmbios => write!(f, "SMBIOS config table has no valid _SM3_ entry point"),
            Error::NoBootHartId => write!(f, "firmware reported no boot HART ID"),
            Error::NoRngSeed => write!(f, "firmware reported no RNG seed"),
            Error::NoKernel => write!(
                f,
                "no kernel file in ESP at path {}",
                crate::kernel::KERNEL_PATH
            ),
            Error::Boot(err) => write!(f, "{err}"),
        }
    }
}
