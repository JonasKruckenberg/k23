use crate::{PhysicalAddress, VirtualAddress};

#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("out of memory")]
    OutOfMemory,
    #[error("Attempted to flush mismatched address space. Expected {expected} but found {found}.")]
    AddressSpaceMismatch { expected: usize, found: usize },
    #[error("attempted to free already freed frame {0:?}")]
    DoubleFree(PhysicalAddress),
    #[cfg(target_arch = "riscv64")]
    #[error("SBI call failed with error {0}")]
    SBI(#[from] kstd::arch::sbi::Error),
    #[error("Address {0:?} is not mapped and cant be translated")]
    NotMapped(VirtualAddress),
}
