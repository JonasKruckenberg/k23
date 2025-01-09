// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! RISC-V CSRs

#![allow(clippy::missing_safety_doc)]

pub mod satp;
pub mod scause;
pub mod scounteren;
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

macro_rules! set {
    ($csr_number: literal) => {
        /// Sets the CSR
        #[inline]
        #[allow(unused_variables)]
        unsafe fn _set(bits: usize) {
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

macro_rules! set_csr_as_usize {
    ($csr_number:literal) => {
        $crate::set!($csr_number);

        /// Sets the CSR.
        ///
        /// **WARNING**: panics on non-`riscv` targets.
        #[inline]
        pub fn set(bits: usize) {
            unsafe { _set(bits) }
        }
    };
}

macro_rules! clear {
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

#[macro_export]
macro_rules! set_csr {
    ($(#[$attr:meta])*, $set_field:ident, $e:expr) => {
        $(#[$attr])*
        #[inline]
        pub unsafe fn $set_field() {
            _set($e);
        }
    };
}

/// Convenience macro to define field clear functions for a CSR type.
#[macro_export]
macro_rules! clear_csr {
    ($(#[$attr:meta])*, $clear_field:ident, $e:expr) => {
        $(#[$attr])*
        #[inline]
        pub unsafe fn $clear_field() {
            _clear($e);
        }
    };
}

#[macro_export]
macro_rules! set_clear_csr {
    ($(#[$attr:meta])*, $set_field:ident, $clear_field:ident, $e:expr) => {
        $crate::set_csr!($(#[$attr])*, $set_field, $e);
        $crate::clear_csr!($(#[$attr])*, $clear_field, $e);
    }
}

pub(crate) use {
    clear, read_composite_csr, read_csr, read_csr_as, read_csr_as_usize, set, set_csr_as_usize,
};
