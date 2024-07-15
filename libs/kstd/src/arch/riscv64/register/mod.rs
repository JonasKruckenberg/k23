#![allow(clippy::missing_safety_doc)]

pub mod satp;
pub mod scause;
pub mod sepc;
pub mod sie;
pub mod sstatus;
pub mod stval;
pub mod stvec;

macro_rules! csr_base_and_read {
    ($ty_name: ident, $csr_name: literal) => {
        #[must_use]
        pub fn read() -> $ty_name {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    let bits: usize;
                    // force $csrname to be a string literal
                    let _csr_name: &str = $csr_name;
                    unsafe {
                        ::core::arch::asm!(concat!("csrr {0}, ", $csr_name), out(reg) bits);
                    }

                    $ty_name { bits }
                } else {
                    unimplemented!()
                }
            }
        }

        pub struct $ty_name {
            bits: usize,
        }

        impl $ty_name {
            /// Returns the contents of the register as raw bits
            #[inline]
            #[must_use]
            pub fn as_bits(&self) -> usize {
                self.bits
            }
        }
    };
}

macro_rules! csr_write {
    ($csr_name: literal) => {
        /// Writes the CSR
        #[inline]
        #[allow(unused_variables)]
        unsafe fn _write(bits: usize) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    let _csr_name: &str = $csr_name;
                    ::core::arch::asm!(concat!("csrrw x0, ", $csr_name, ", {0}"), in(reg) bits)
                } else {
                    unimplemented!()
                }
            }
        }
    };
}

macro_rules! csr_clear {
    ($csr_name: literal) => {
        /// Writes the CSR
        #[inline]
        #[allow(unused_variables)]
        unsafe fn _clear(bits: usize) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    let _csr_name: &str = $csr_name;
                    ::core::arch::asm!(concat!("csrrc x0, ", $csr_name, ", {0}"), in(reg) bits)
                } else {
                    unimplemented!()
                }
            }
        }
    };
}

pub(crate) use {csr_base_and_read, csr_clear, csr_write};
