//! Supervisor Status Register

use super::{clear_csr, read_csr_as, write_csr};
use core::fmt;
use core::fmt::Formatter;

/// Supervisor Status Register
#[derive(Clone, Copy)]
pub struct Sstatus {
    bits: usize,
}

read_csr_as!(Sstatus, 0x100);
write_csr!(0x100);
clear_csr!(0x100);

/// Supervisor Interrupt Enable
pub unsafe fn set_sie() {
    _write(1 << 1);
}

/// Supervisor Interrupt Enable
pub unsafe fn clear_sie() {
    _clear(1 << 1);
}

/// Supervisor Previous Interrupt Enable
pub unsafe fn set_spie() {
    _write(1 << 5);
}

/// Supervisor Previous Privilege Mode
#[inline]
pub unsafe fn set_spp(spp: SPP) {
    match spp {
        SPP::Supervisor => _write(1 << 8),
        SPP::User => _clear(1 << 8),
    }
}

/// Floating-Point Unit Status
pub unsafe fn set_fs(fs: FS) {
    let mut value = read().bits;
    value &= !(0x3 << 13); // clear previous value
    value |= (fs as usize) << 13;
    _write(value);
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SPP {
    Supervisor = 1,
    User = 0,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FS {
    Off = 0,
    Initial = 1,
    Clean = 2,
    Dirty = 3,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum XLEN {
    XLEN32,
    XLEN64,
    XLEN128,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Endianness {
    LittleEndian,
    BigEndian,
}

impl Sstatus {
    /// Supervisor Interrupt Enable
    #[inline]
    #[must_use]
    pub fn sie(&self) -> bool {
        self.bits & (1 << 1) != 0
    }

    /// Supervisor Previous Interrupt Enable
    #[inline]
    #[must_use]
    pub fn spie(&self) -> bool {
        self.bits & (1 << 5) != 0
    }

    #[inline]
    #[must_use]
    pub fn ube(&self) -> Endianness {
        match self.bits & (1 << 6) {
            0 => Endianness::LittleEndian,
            1 => Endianness::BigEndian,
            _ => unreachable!(),
        }
    }

    /// Supervisor Previous Privilege Mode
    #[inline]
    #[must_use]
    pub fn spp(&self) -> SPP {
        if self.bits & (1 << 8) != 0 {
            SPP::Supervisor
        } else {
            SPP::User
        }
    }

    /// The status of the vector unit
    #[inline]
    #[must_use]
    pub fn vs(&self) -> FS {
        let fs = (self.bits >> 9) & 0x3; // bits 13-14
        match fs {
            0 => FS::Off,
            1 => FS::Initial,
            2 => FS::Clean,
            3 => FS::Dirty,
            _ => unreachable!(),
        }
    }

    /// The status of the floating-point unit
    #[inline]
    #[must_use]
    pub fn fs(&self) -> FS {
        let fs = (self.bits >> 13) & 0x3; // bits 13-14
        match fs {
            0 => FS::Off,
            1 => FS::Initial,
            2 => FS::Clean,
            3 => FS::Dirty,
            _ => unreachable!(),
        }
    }

    /// The status of additional user-mode extensions
    /// and associated state
    #[inline]
    #[must_use]
    pub fn xs(&self) -> FS {
        let xs = (self.bits >> 15) & 0x3; // bits 15-16
        match xs {
            0 => FS::Off,
            1 => FS::Initial,
            2 => FS::Clean,
            3 => FS::Dirty,
            _ => unreachable!(),
        }
    }

    /// Permit Supervisor User Memory access
    #[inline]
    #[must_use]
    pub fn sum(&self) -> bool {
        self.bits & (1 << 18) != 0
    }

    /// Make eXecutable Readable
    #[inline]
    #[must_use]
    pub fn mxr(&self) -> bool {
        self.bits & (1 << 19) != 0
    }

    /// Effective xlen in U-mode (i.e., `UXLEN`).
    ///
    /// In RISCV-32, UXL does not exist, and `UXLEN` is always [`XLEN::XLEN32`].
    #[inline]
    #[must_use]
    pub fn uxl(&self) -> XLEN {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "riscv32")] {
                XLEN::XLEN32
            } else {
                #[allow(clippy::cast_possible_truncation)] // We actually want to truncate
                match (self.bits >> 32) as u8 & 0x3 {
                    1 => XLEN::XLEN32,
                    2 => XLEN::XLEN64,
                    3 => XLEN::XLEN128,
                    _ => unreachable!(),
                }
            }
        }
    }

    /// Whether either the FS field or XS field
    /// signals the presence of some dirty state
    #[inline]
    #[must_use]
    pub fn sd(&self) -> bool {
        self.bits & (1 << (usize::BITS as usize - 1)) != 0
    }
}

impl fmt::Debug for Sstatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sstatus")
            .field("sie", &self.sie())
            .field("spie", &self.spie())
            .field("ube", &self.ube())
            .field("spp", &self.spp())
            .field("vs", &self.vs())
            .field("fs", &self.fs())
            .field("xs", &self.xs())
            .field("sum", &self.sum())
            .field("mxr", &self.mxr())
            .field("uxl", &self.uxl())
            .field("sd", &self.sd())
            .finish()
    }
}
