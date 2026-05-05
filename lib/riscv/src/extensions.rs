// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::str::FromStr;

bitflags::bitflags! {
    /// Known RISC-V extensions.
    ///
    /// Note that this is *not* the complete set of all standardized RISC-V extensions, merely the
    /// subset of extensions we care about. If we add features conditional on extensions, we should
    /// add them here.
    #[derive(Debug, Default, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct RiscvExtensions: u16 {
        /// Base ISA
        const I = 1 << 0;
        /// Integer Multiplication and Division
        const M = 1 << 1;
        /// Atomic Instructions
        const A = 1 << 2;
        /// Single-Precision Floating-Point
        const F = 1 << 3;
        /// Double-Precision Floating-Point
        const D = 1 << 4;
        /// Compressed Instructions
        const C = 1 << 5;
        /// Control and Status Register Access
        const ZICSR = 1 << 6;
        /// Basic Performance Counters
        const ZICNTR = 1 << 7;
        /// Main memory supports instruction fetch with atomicity requirement
        ///
        /// Main memory regions with both the cacheability and coherence PMAs must support
        /// instruction fetch, and any instruction fetches of naturally aligned power-of-2 sizes up to
        /// min(ILEN,XLEN) (i.e., 32 bits for RVA20) are atomic.
        const ZICCIF = 1 << 8;
        /// Main memory supports forward progress on LR/SC sequences.
        ///
        /// Main memory regions with both the cacheability and coherence PMAs must support
        /// RsrvEventual.
        const ZICCRSE = 1 << 9;
        /// Main memory supports all atomics in A.
        ///
        /// Main memory regions with both the cacheability and coherence PMAs must support
        /// AMOArithmetic.
        const ZICCAMOA = 1 << 10;
        /// Reservation set size of at most 128 bytes.
        ///
        /// Reservation sets must be contiguous, naturally aligned, and at most 128 bytes in size.
        const ZA128RS = 1 << 11;
        /// Main memory supports misaligned loads/stores.
        ///
        /// Misaligned loads and stores to main memory regions with both the cacheability and
        /// coherence PMAs must be supported.
        const ZICCLSM = 1 << 12;
        /// Hardware Performance Counters
        const ZIHPM = 1 << 13;

        /// RVA20U64 profile
        const RVA20U64 = Self::I.bits()
                | Self::M.bits()
                | Self::A.bits()
                | Self::F.bits()
                | Self::D.bits()
                | Self::C.bits()
                | Self::ZICSR.bits()
                | Self::ZICNTR.bits()
                | Self::ZICCIF.bits()
                | Self::ZICCRSE.bits()
                | Self::ZICCAMOA.bits()
                | Self::ZA128RS.bits()
                | Self::ZICCLSM.bits();
    }
}

impl FromStr for RiscvExtensions {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "i" => Ok(Self::I),
            "m" => Ok(Self::M),
            "a" => Ok(Self::A),
            "f" => Ok(Self::F),
            "d" => Ok(Self::D),
            "c" => Ok(Self::C),
            "zicsr" => Ok(Self::ZICSR),
            "zicntr" => Ok(Self::ZICNTR),
            "ziccif" => Ok(Self::ZICCIF),
            "ziccrse" => Ok(Self::ZICCRSE),
            "ziccamoa" => Ok(Self::ZICCAMOA),
            "za128rs" => Ok(Self::ZA128RS),
            "zicclsm" => Ok(Self::ZICCLSM),
            "zihpm" => Ok(Self::ZIHPM),
            _ => Err(()),
        }
    }
}

impl fmt::Display for RiscvExtensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}
