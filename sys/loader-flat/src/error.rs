// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    Fdt(fdt::Error),
    Common(loader_common::Error),

    /// Could not determine the boot HART id from any source.
    NoBootHartId,
    NoRngSeed,
}

impl From<fdt::Error> for Error {
    fn from(err: fdt::Error) -> Self {
        Self::Fdt(err)
    }
}

impl From<loader_common::Error> for Error {
    fn from(err: loader_common::Error) -> Self {
        Self::Common(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Fdt(err) => write!(f, "FDT parse error: {err}"),
            Error::Common(err) => err.fmt(f),
            Error::NoBootHartId => write!(f, "firmware reported no boot HART ID"),
            Error::NoRngSeed => write!(f, "firmware reported no RNG seed"),
        }
    }
}
