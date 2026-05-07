// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem::size_of;

use zerocopy::byteorder::{BigEndian, LittleEndian, U16, U32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::validate::Validate;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BothEndianU16 {
    le: U16<LittleEndian>,
    be: U16<BigEndian>,
}
const _: () = assert!(size_of::<BothEndianU16>() == 4);

impl BothEndianU16 {
    pub fn new(n: u16) -> Self {
        Self {
            le: U16::new(n),
            be: U16::new(n),
        }
    }

    #[cfg(target_endian = "little")]
    pub fn get(self) -> u16 {
        self.le.get()
    }

    #[cfg(target_endian = "big")]
    pub fn get(self) -> u16 {
        self.be.get()
    }
}

impl Validate for BothEndianU16 {
    fn validate(&self) -> anyhow::Result<()> {
        let le = u64::from(self.le.get());
        let be = u64::from(self.be.get());
        anyhow::ensure!(
            le == be,
            "BothEndianU16: LE/BE mismatch (LE={le:#x} BE={be:#x})"
        );
        Ok(())
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BothEndianU32 {
    le: U32<LittleEndian>,
    be: U32<BigEndian>,
}
const _: () = assert!(size_of::<BothEndianU32>() == 8);

impl BothEndianU32 {
    pub fn new(n: u32) -> Self {
        Self {
            le: U32::new(n),
            be: U32::new(n),
        }
    }

    #[cfg(target_endian = "little")]
    pub fn get(self) -> u32 {
        self.le.get()
    }

    #[cfg(target_endian = "big")]
    pub fn get(self) -> u32 {
        self.be.get()
    }
}

impl Validate for BothEndianU32 {
    fn validate(&self) -> anyhow::Result<()> {
        let le = u64::from(self.le.get());
        let be = u64::from(self.be.get());
        anyhow::ensure!(
            le == be,
            "BothEndianU32: LE/BE mismatch (LE={le:#x} BE={be:#x})"
        );
        Ok(())
    }
}
