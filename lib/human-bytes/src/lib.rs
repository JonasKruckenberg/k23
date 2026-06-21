// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]

use core::fmt::{self, Display};

/// One kibibyte (2^10 bytes).
pub const KIB: usize = 1024;
/// One mebibyte (2^20 bytes).
pub const MIB: usize = KIB * 1024;
/// One gibibyte (2^30 bytes).
pub const GIB: usize = MIB * 1024;
/// One tebibyte (2^40 bytes).
#[cfg(target_pointer_width = "64")]
pub const TIB: usize = GIB * 1024;

/// Binary byte units
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    B = 0,
    KiB = 1,
    MiB = 2,
    GiB = 3,
    TiB = 4,
    PiB = 5,
    EiB = 6,
}

impl Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unit::B => f.write_str("B"),
            Unit::KiB => f.write_str("KiB"),
            Unit::MiB => f.write_str("MiB"),
            Unit::GiB => f.write_str("GiB"),
            Unit::TiB => f.write_str("TiB"),
            Unit::PiB => f.write_str("PiB"),
            Unit::EiB => f.write_str("EiB"),
        }
    }
}

/// Formats a byte count in a human-readable way (e.g. `1.50 MiB`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HumanBytes {
    pub whole: usize,
    pub frac: u8,
    pub unit: Unit,
}

impl HumanBytes {
    /// Create a human-readable representation from the given `bytes`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "truncations are on purpose"
    )]
    pub fn from(bytes: usize) -> Self {
        if bytes == 0 {
            return Self {
                whole: 0,
                frac: 0,
                unit: Unit::B,
            };
        }

        let unit = (bytes.ilog2() / 10) as u8;

        let shift = 10 * unit;
        let whole = bytes >> shift;
        let frac = (((bytes & ((1usize << shift) - 1)).saturating_mul(100)) >> shift) as u8;

        Self {
            whole,
            frac,
            unit: match unit {
                0 => Unit::B,
                1 => Unit::KiB,
                2 => Unit::MiB,
                3 => Unit::GiB,
                4 => Unit::TiB,
                5 => Unit::PiB,
                6 => Unit::EiB,
                _ => unreachable!(),
            },
        }
    }
}

impl fmt::Display for HumanBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.unit {
            Unit::B => write!(f, "{} B", self.whole),
            _ => write!(f, "{}.{:02} {}", self.whole, self.frac, self.unit),
        }
    }
}
