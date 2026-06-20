// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    Elf(object::Error),
    Alloc(mem_core::AllocError),
    Fdt(fdt::Error),
    /// A `PT_LOAD` segment carries a `p_flags` combination the loader does not
    /// support (anything other than `R`, `R|W`, or `R|X`).
    InvalidSegmentFlags(u32),
    TooManyRegions,
    MissingSegment,
    FieldOutOfRange,
    MalformedImage,
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

impl From<core::num::TryFromIntError> for Error {
    fn from(_err: core::num::TryFromIntError) -> Self {
        Self::FieldOutOfRange
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
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
            Error::TooManyRegions => {
                write!(f, "firmware reported too many physical memory regions")
            }
            Error::MissingSegment => write!(f, "missing required section"),
            Error::FieldOutOfRange => write!(f, "ELF/firmware field out of range"),
            Error::MalformedImage => write!(f, "kernel ELF or debug-info file malformed"),
        }
    }
}

#[macro_export]
macro_rules! ensure {
    ($cond:expr, $err:expr) => {{
        if !$cond {
            log::error!("expected {} == true", stringify!($cond));
            return Err($err);
        }
    }};
}
