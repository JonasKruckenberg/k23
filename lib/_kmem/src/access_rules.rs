// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mycelium_bitfield::bitfield! {
    /// Rules that dictate how a region of virtual memory may be accessed.
    ///
    /// # W^X
    ///
    /// In order to prevent malicious code execution as proactively as possible,
    /// [`AccessRules`] can either allow *writes* OR *execution* but never both. This is enforced
    /// through the [`WriteOrExecute`] enum field.
    #[derive(PartialEq, Eq)]
    pub struct AccessRules<u8> {
        /// If set, reading from the memory region is allowed.
        pub const READ: bool;
        /// Whether executing, or writing this memory region is allowed (or neither).
        pub const WRITE_OR_EXECUTE: WriteOrExecute;
        /// If set, requires code in the memory region to use aarch64 Branch Target Identification.
        /// Does nothing on non-aarch64 architectures.
        pub const BTI: bool;
    }
}

/// Whether executing, or writing this memory region is allowed (or neither).
///
/// This is an enum to enforce [`W^X`] at the type-level.
///
/// [`W^X`]: AccessRules
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum WriteOrExecute {
    /// Neither writing nor execution of the memory region is allowed.
    Neither = 0b00,
    /// Writing to the memory region is allowed.
    Write = 0b01,
    /// Executing code from the memory region is allowed.
    Execute = 0b10,
}

// ===== impl AccessRules =====

impl AccessRules {
    pub const fn is_read_only(&self) -> bool {
        const READ_MASK: u8 = AccessRules::READ.max_value();
        self.0 & READ_MASK == 1
    }

    pub fn allows_read(&self) -> bool {
        self.get(Self::READ)
    }

    pub fn allows_write(&self) -> bool {
        matches!(self.get(Self::WRITE_OR_EXECUTE), WriteOrExecute::Write)
    }

    pub fn allows_execution(&self) -> bool {
        matches!(self.get(Self::WRITE_OR_EXECUTE), WriteOrExecute::Execute)
    }
}

// ===== impl WriteOrExecute =====

impl mycelium_bitfield::FromBits<u8> for WriteOrExecute {
    type Error = core::convert::Infallible;

    /// The number of bits required to represent a value of this type.
    const BITS: u32 = 2;

    #[inline]
    fn try_from_bits(bits: u8) -> Result<Self, Self::Error> {
        match bits {
            b if b == Self::Neither as u8 => Ok(Self::Neither),
            b if b == Self::Write as u8 => Ok(Self::Write),
            b if b == Self::Execute as u8 => Ok(Self::Execute),
            _ => {
                // this should never happen unless the bitpacking code is broken
                unreachable!("invalid memory region access rules {bits:#b}")
            }
        }
    }

    #[inline]
    fn into_bits(self) -> u8 {
        self as u8
    }
}
