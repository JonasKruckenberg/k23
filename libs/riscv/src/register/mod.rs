//! RISC-V CSRs

#![allow(clippy::missing_safety_doc)]

pub mod satp;
pub mod scause;
pub mod sepc;
pub mod sie;
pub mod sstatus;
pub mod stval;
pub mod stvec;
pub mod time;

macro_rules! read_csr {
    ($csr_number:literal) => {
        /// Reads the CSR.
        ///
        /// **WARNING**: panics on non-`riscv` targets.
        #[inline]
        unsafe fn _read() -> usize {
            match () {
                #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
                () => {
                    let r: usize;
                    core::arch::asm!(concat!("csrrs {0}, ", stringify!($csr_number), ", x0"), out(reg) r);
                    r
                }

                #[cfg(not(any(target_arch = "riscv32", target_arch = "riscv64")))]
                () => unimplemented!()
            }
        }
    };
}

macro_rules! read_csr_as {
    ($register:ident, $csr_number:literal) => {
        $crate::read_csr!($csr_number);

        /// Reads the CSR.
        ///
        /// **WARNING**: panics on non-`riscv` targets.
        #[inline]
        pub fn read() -> $register {
            $register {
                bits: unsafe { _read() },
            }
        }
    };
}

macro_rules! read_csr_as_usize {
    ($csr_number:literal) => {
        $crate::read_csr!($csr_number);

        /// Reads the CSR.
        ///
        /// **WARNING**: panics on non-`riscv` targets.
        #[inline]
        pub fn read() -> usize {
            unsafe { _read() }
        }
    };
}

macro_rules! read_composite_csr {
    ($hi:expr, $lo:expr) => {
        /// Reads the CSR as a 64-bit value
        #[inline]
        pub fn read64() -> u64 {
            match () {
                #[cfg(target_arch = "riscv32")]
                () => loop {
                    let hi = $hi;
                    let lo = $lo;
                    if hi == $hi {
                        return ((hi as u64) << 32) | lo as u64;
                    }
                },

                #[cfg(not(target_arch = "riscv32"))]
                () => $lo as u64,
            }
        }
    };
}

macro_rules! write_csr {
    ($csr_number: literal) => {
        /// Writes the CSR
        #[inline]
        #[allow(unused_variables)]
        unsafe fn _write(bits: usize) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    core::arch::asm!(concat!("csrrw x0, ", stringify!($csr_number), ", {0}"), in(reg) bits);
                } else {
                    unimplemented!()
                }
            }
        }
    };
}

macro_rules! write_csr_as_usize {
    ($csr_number:literal) => {
        $crate::write_csr!($csr_number);

        /// Writes the CSR.
        ///
        /// **WARNING**: panics on non-`riscv` targets.
        #[inline]
        pub fn write(bits: usize) {
            unsafe { _write(bits) }
        }
    };
}

macro_rules! clear_csr {
    ($csr_number: literal) => {
        /// Writes the CSR
        #[inline]
        #[allow(unused_variables)]
        unsafe fn _clear(bits: usize) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    ::core::arch::asm!(concat!("csrrc x0, ", stringify!($csr_number), ", {0}"), in(reg) bits)
                } else {
                    unimplemented!()
                }
            }
        }
    };
}

pub(crate) use {
    clear_csr, read_composite_csr, read_csr, read_csr_as, read_csr_as_usize, write_csr,
    write_csr_as_usize,
};
