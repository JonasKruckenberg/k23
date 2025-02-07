// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! RISC-V CSRs

#![expect(
    clippy::missing_safety_doc,
    clippy::undocumented_unsafe_blocks,
    reason = "register access"
)]

pub mod satp;
pub mod scause;
pub mod scounteren;
pub mod sepc;
pub mod sie;
pub mod sip;
pub mod sscratch;
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
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    unsafe {
                        let r: usize;
                        ::core::arch::asm!(concat!("csrrs {0}, ", stringify!($csr_number), ", x0"), out(reg) r);
                        r
                    }
                } else {
                    unimplemented!()
                }
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
            // Safety: register access
            unsafe { _read() }
        }
    };
}

macro_rules! read_composite_csr {
    ($hi:expr, $lo:expr) => {
        /// Reads the CSR as a 64-bit value
        #[inline]
        pub fn read64() -> u64 {
            cfg_if::cfg_if! {
                if #[cfg(target_arch = "riscv64")] {
                   $lo as u64
                } else if #[cfg(target_arch = "riscv32")] {
                    let hi = $hi;
                    let lo = $lo;
                    if hi == $hi {
                        return ((hi as u64) << 32) | lo as u64;
                    }
                } else {
                    unimplemented!()
                }
            }
        }
    };
}

macro_rules! write_csr {
    ($csr_number: literal) => {
        /// Writes the CSR
        #[inline]
        unsafe fn _write(bits: usize) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    unsafe {
                        ::core::arch::asm!(concat!("csrrw x0, ", stringify!($csr_number), ", {0}"), in(reg) bits);
                    }
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

        /// Sets the CSR.
        ///
        /// **WARNING**: panics on non-`riscv` targets.
        #[inline]
        pub fn set(bits: usize) {
            unsafe { _write(bits) }
        }
    };
}

macro_rules! set_csr {
    ($csr_number: literal) => {
        /// Sets the CSR
        #[inline]
        unsafe fn _set(bits: usize) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    unsafe {
                        ::core::arch::asm!(concat!("csrrs x0, ", stringify!($csr_number), ", {0}"), in(reg) bits);
                    }
                } else {
                    unimplemented!()
                }
            }
        }
    };
}

macro_rules! clear_csr {
    ($csr_number: literal) => {
        /// Writes the CSR
        #[inline]
        unsafe fn _clear(bits: usize) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    unsafe {
                        ::core::arch::asm!(concat!("csrrc x0, ", stringify!($csr_number), ", {0}"), in(reg) bits)
                    }
                } else {
                    unimplemented!()
                }
            }
        }
    };
}

macro_rules! set_csr_field {
    ($(#[$attr:meta])*, $set_field:ident, $e:expr) => {
        $(#[$attr])*
        #[inline]
        pub unsafe fn $set_field() {
            let e =  $e;
            unsafe { _set(e); }
        }
    };
}

macro_rules! clear_csr_field {
    ($(#[$attr:meta])*, $clear_field:ident, $e:expr) => {
        $(#[$attr])*
        #[inline]
        pub unsafe fn $clear_field() {
            let e = $e;
            unsafe { _clear(e); }
        }
    };
}

macro_rules! set_clear_csr_field {
    ($(#[$attr:meta])*, $set_field:ident, $clear_field:ident, $e:expr) => {
        $crate::set_csr_field!($(#[$attr])*, $set_field, $e);
        $crate::clear_csr_field!($(#[$attr])*, $clear_field, $e);
    }
}

pub(crate) use {
    clear_csr, clear_csr_field, read_composite_csr, read_csr, read_csr_as, read_csr_as_usize,
    set_clear_csr_field, set_csr, set_csr_field, write_csr, write_csr_as_usize,
};
