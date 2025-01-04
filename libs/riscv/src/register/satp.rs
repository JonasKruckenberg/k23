//! Supervisor Address Translation and Protection Register

use super::{read_csr_as, set};
use crate::Error;
use core::fmt;

/// satp register
#[derive(Clone, Copy)]
pub struct Satp {
    bits: usize,
}

read_csr_as!(Satp, 0x180);
set!(0x180);

/// Sets the register to corresponding page table mode, physical page number and address space id.
///
/// **WARNING**: panics on:
///
/// - non-`riscv` targets
/// - invalid field values
#[inline]
#[cfg(target_pointer_width = "32")]
pub unsafe fn set(mode: Mode, asid: usize, ppn: usize) {
    try_set(mode, asid, ppn).unwrap();
}

/// Attempts to set the register to corresponding page table mode, physical page number and address space id.
#[inline]
#[cfg(target_pointer_width = "32")]
pub unsafe fn try_set(mode: Mode, asid: usize, ppn: usize) -> Result<()> {
    if asid != asid & 0x1FF {
        Err(Error::InvalidFieldValue {
            field: "asid",
            value: asid,
            bitmask: 0x1FF,
        })
    } else if ppn != ppn & 0x3F_FFFF {
        Err(Error::InvalidFieldValue {
            field: "ppn",
            value: ppn,
            bitmask: 0x3F_FFFF,
        })
    } else {
        let bits = (mode as usize) << 31 | (asid << 22) | ppn;
        _set(bits)
    }
}

/// Sets the register to corresponding page table mode, physical page number and address space id.
///
/// **WARNING**: panics on:
///
/// - non-`riscv` targets
/// - invalid field values
#[inline]
#[cfg(target_pointer_width = "64")]
pub unsafe fn set(mode: Mode, asid: usize, ppn: usize) {
    try_set(mode, asid, ppn).unwrap()
}

/// Attempts to set the register to corresponding page table mode, physical page number and address space id.
#[inline]
#[cfg(target_pointer_width = "64")]
pub unsafe fn try_set(mode: Mode, asid: usize, ppn: usize) -> crate::Result<()> {
    if asid != asid & 0xFFFF {
        Err(Error::InvalidFieldValue {
            field: "asid",
            value: asid,
            bitmask: 0xFFFF,
        })
    } else if ppn != ppn & 0xFFF_FFFF_FFFF {
        Err(Error::InvalidFieldValue {
            field: "ppn",
            value: ppn,
            bitmask: 0xFFF_FFFF_FFFF,
        })
    } else {
        let bits = (mode as usize) << 60 | (asid << 44) | ppn;
        _set(bits);
        Ok(())
    }
}
impl Satp {
    #[cfg(target_arch = "riscv32")]
    pub fn ppn(&self) -> usize {
        self.bits & 0x3f_ffff // bits 0-21
    }
    #[cfg(target_arch = "riscv64")]
    #[must_use]
    pub fn ppn(&self) -> usize {
        self.bits & 0xfff_ffff_ffff // bits 0-43
    }
    #[cfg(target_arch = "riscv32")]
    pub fn asid(&self) -> usize {
        (self.bits >> 22) & 0x1ff // bits 22-30
    }
    #[cfg(target_arch = "riscv64")]
    #[must_use]
    pub fn asid(&self) -> usize {
        (self.bits >> 44) & 0xffff // bits 44-60
    }
    #[cfg(target_arch = "riscv32")]
    pub fn mode(&self) -> Mode {
        match (self.bits >> 31) != 0 {
            true => Mode::Sv32,
            false => Mode::Bare,
        }
    }
    #[cfg(target_arch = "riscv64")]
    #[must_use]
    pub fn mode(&self) -> Mode {
        // bits 60-64
        match (self.bits >> 60) & 0xf {
            0 => Mode::Bare,
            8 => Mode::Sv39,
            9 => Mode::Sv48,
            10 => Mode::Sv57,
            11 => Mode::Sv64,
            _ => unreachable!(),
        }
    }
}

#[cfg(target_pointer_width = "32")]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Mode {
    Bare = 0,
    Rv32 = 1,
}

#[cfg(target_pointer_width = "64")]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Mode {
    Bare = 0,
    Sv39 = 8,
    Sv48 = 9,
    Sv57 = 10,
    Sv64 = 11,
}

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
impl fmt::Debug for Satp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Satp")
            .field("ppn", &self.ppn())
            .field("asid", &self.asid())
            .field("mode", &self.mode())
            .finish()
    }
}
