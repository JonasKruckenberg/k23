// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Supervisor Address Translation and Protection Register

use super::{read_csr_as, write_csr};
use crate::Error;

/// satp register
#[derive(Clone, Copy)]
#[cfg_attr(
    not(any(target_arch = "riscv64", target_arch = "riscv32")),
    allow(unused)
)]
pub struct Satp {
    bits: usize,
}

read_csr_as!(Satp, 0x180);
write_csr!(0x180);

/// Sets the register to corresponding page table mode, physical page number and address space id.
///
/// **WARNING**: panics on:
///
/// - non-`riscv` targets
/// - invalid field values
#[inline]
#[cfg(target_pointer_width = "32")]
pub unsafe fn set(mode: Mode, asid: u16, ppn: usize) {
    try_set(mode, asid, ppn).unwrap();
}

/// Attempts to set the register to corresponding page table mode, physical page number and address space id.
#[inline]
#[cfg(target_pointer_width = "32")]
pub unsafe fn try_set(mode: Mode, asid: u16, ppn: usize) -> Result<()> {
    if asid != asid & 0x1FF {
        Err(Error::InvalidFieldValue {
            field: "asid",
            value: asid as usize,
            bitmask: 0x1FF,
        })
    } else if ppn != ppn & 0x3F_FFFF {
        Err(Error::InvalidFieldValue {
            field: "ppn",
            value: ppn,
            bitmask: 0x3F_FFFF,
        })
    } else {
        let bits = (mode as usize) << 31 | ((asid as usize) << 22) | ppn;
        _set(bits)
    }
}

/// Sets the register to corresponding page table mode, physical page number and address space id.
///
/// # Panics
///
/// - panics on non-`riscv` targets
/// - panics on invalid field values
#[inline]
#[cfg(target_pointer_width = "64")]
pub unsafe fn set(mode: Mode, asid: u16, ppn: usize) {
    unsafe { try_set(mode, asid, ppn).unwrap() }
}

/// Attempts to set the register to corresponding page table mode, physical page number and address space id.
///
/// # Errors
///
/// Returns an error if the values are out of range for their fields.
#[inline]
#[cfg(target_pointer_width = "64")]
pub unsafe fn try_set(mode: Mode, asid: u16, ppn: usize) -> crate::Result<()> {
    if ppn != ppn & 0xFFF_FFFF_FFFF {
        Err(Error::InvalidFieldValue {
            field: "ppn",
            value: ppn,
            bitmask: 0xFFF_FFFF_FFFF,
        })
    } else {
        let bits = (mode as usize) << 60 | ((asid as usize) << 44) | ppn;
        unsafe {
            _write(bits);
        }
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
    pub fn asid(&self) -> u16 {
        // Safety: `& 0xffff` ensures the number must be 16 bit
        unsafe { u16::try_from((self.bits >> 44) & 0xffff).unwrap_unchecked() } // bits 44-60
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
impl core::fmt::Debug for Satp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Satp")
            .field("ppn", &self.ppn())
            .field("asid", &self.asid())
            .field("mode", &self.mode())
            .finish()
    }
}
