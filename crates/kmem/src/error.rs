use crate::{PhysicalAddress, VirtualAddress};

#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("SBI call failed with error {0}")]
    SBI(#[from] sbicall::Error),
    #[error("out of memory")]
    OutOfMemory,
    #[error("Address out of bounds")]
    AddressOutOfBounds,
    #[error("page table index is out of bounds, expected 0..512 but found {0}")]
    PageIndexOutOfBounds(usize),
    #[error("Attempted to flush mismatched address space. Expected {expected} but found {found}.")]
    AddressSpaceMismatch { expected: usize, found: usize },
    #[error("can only map to aligned physical addresses ({0:?})")]
    PhysicalAddressAlignment(PhysicalAddress),
    #[error("can only map to aligned virtual addresses ({0:?})")]
    VirtualAddressAlignment(VirtualAddress),
    #[error("the given combination of page flags is invalid")]
    InvalidPageFlags,
}

macro_rules! ensure {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err($err);
        }
    };
}

pub(crate) use ensure;
