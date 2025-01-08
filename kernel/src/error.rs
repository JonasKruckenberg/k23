use core::fmt::{Display, Formatter};
use crate::vm::frame_alloc;

#[derive(Debug)]
pub enum Error {
    /// Failed to set up mappings
    Mmu(mmu::Error),
    /// Failed to parse device tree blob
    Dtb(dtb_parser::Error),
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
}

impl From<mmu::Error> for Error {
    fn from(err: mmu::Error) -> Self {
        Self::Mmu(err)
    }
}

impl From<dtb_parser::Error> for Error {
    fn from(err: dtb_parser::Error) -> Self {
        Self::Dtb(err)
    }
}

impl From<frame_alloc::AllocError> for Error {
    fn from(_value: frame_alloc::AllocError) -> Self {
        Self::NoResources
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Mmu(_) => f.write_str("Failed to set up mappings"),
            Error::Dtb(_) => f.write_str("Failed to parse device tree blob"),
            Error::AccessDenied => {
                f.write_str("The caller did not have permission to perform the specified operation")
            }
            Error::InvalidArgument => f.write_str("An argument is invalid"),
            Error::AlreadyExists => f.write_str(
                "An object with the specified identifier or at the specified place already exists",
            ),
            Error::NoResources => f.write_str(
                "The system was not able to allocate some resource needed for the operation",
            ),
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
}

#[macro_export]
macro_rules! bail {
    ($error:expr, $msg:expr) => {
        log::error!($msg);
        return Err($error);
    };
}
