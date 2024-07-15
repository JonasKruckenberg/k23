use super::{csr_base_and_read, csr_write};
use core::fmt;
use core::fmt::Formatter;

csr_base_and_read!(Satp, "satp");
csr_write!("satp");

#[cfg(target_arch = "riscv32")]
pub unsafe fn set(mode: Mode, asid: usize, ppn: usize) {
    let mode = match mode {
        Mode::Bare => 0,
        Mode::Sv32 => 1,
    };

    _write(ppn | (asid << 22) | (mode << 31))
}

#[cfg(target_arch = "riscv64")]
pub unsafe fn set(mode: Mode, asid: usize, ppn: usize) {
    let mode = match mode {
        Mode::Bare => 0,
        Mode::Sv39 => 8,
        Mode::Sv48 => 9,
        Mode::Sv57 => 10,
        Mode::Sv64 => 11,
    };

    _write(ppn | (asid << 44) | (mode << 60));
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

#[cfg(target_arch = "riscv32")]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Mode {
    Bare = 0,
    Rv32 = 1,
}

#[cfg(target_arch = "riscv64")]
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
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Satp")
            .field("ppn", &self.ppn())
            .field("asid", &self.asid())
            .field("mode", &self.mode())
            .finish()
    }
}
