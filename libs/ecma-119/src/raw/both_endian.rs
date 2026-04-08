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
        let le = self.le.get() as u64;
        let be = self.be.get() as u64;
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
        let le = self.le.get() as u64;
        let be = self.be.get() as u64;
        anyhow::ensure!(
            le == be,
            "BothEndianU32: LE/BE mismatch (LE={le:#x} BE={be:#x})"
        );
        Ok(())
    }
}
