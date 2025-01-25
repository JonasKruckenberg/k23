// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! SBI (RISC-V Supervisor Binary Interface).
//!
//! SBI is a standard interface for interacting with the "supervisor execution environment" on RISC-V.
//! This environment provided by the previous stage bootloader (most commonly OpenSBI) is responsible for
//! implementing the SBI functions.
//!
//! You can think of the "supervisor execution environment" as a minimal operating system,
//! running in M-mode that provides services to the operating system running in S-mode.

pub mod base;
pub mod dbcn;
mod error;
pub mod hsm;
pub mod ipi;
pub mod rfence;
pub mod time;

const EID_BASE: usize = 0x10;
const EID_TIME: usize = 0x54494D45;
const EID_IPI: usize = 0x735049;
const EID_RFNC: usize = 0x52464E43;
const EID_HSM: usize = 0x48534D;
const EID_SRST: usize = 0x53525354;
const EID_PMU: usize = 0x504D55;
const EID_DBCN: usize = 0x4442434E;
const EID_SUSP: usize = 0x53555350;
const EID_CPPC: usize = 0x43505043;
const EID_NACL: usize = 0x4E41434C;
const EID_STA: usize = 0x535441;
const EID_SSE: usize = 0x535345;
const EID_FWFT: usize = 0x46574654;
const EID_DBTR: usize = 0x44425452;
const EID_MPXY: usize = 0x4D505859;

use bitflags::bitflags;
pub use error::Error;

type Result<T> = core::result::Result<T, Error>;

macro_rules! sbi_call {
    (ext: $ext:expr, func: $func:expr) => {{
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                let error: isize;
                let value: usize;
                let ext: usize = $ext;
                let func: usize = $func;

                // Safety: inline assembly
                unsafe {
                    ::core::arch::asm!(
                        "ecall",
                        in("a6") func, in("a7") ext,
                        lateout("a0") error, lateout("a1") value,
                    )
                };

                if error == 0 {
                    Ok(value)
                } else {
                    match error {
                        -1 => Err($crate::sbi::Error::Failed),
                        -2 => Err($crate::sbi::Error::NotSupported),
                        -3 => Err($crate::sbi::Error::InvalidParam),
                        -4 => Err($crate::sbi::Error::Denied),
                        -5 => Err($crate::sbi::Error::InvalidAddress),
                        -6 => Err($crate::sbi::Error::AlreadyAvailable),
                        -7 => Err($crate::sbi::Error::AlreadyStarted),
                        -8 => Err($crate::sbi::Error::AlreadyStopped),
                        -9 => Err($crate::sbi::Error::NoShmem),
                        code => Err($crate::sbi::Error::Other(code)),
                    }
                }
            }  else {
                let _: usize = $ext;
                let _: usize = $func;

                #[inline(always)]
                fn unimplemented() -> super::Result<usize> {
                    unimplemented!()
                }
                unimplemented()
            }
        }
    }};
    (ext: $ext:expr, func: $func:expr, $($reg:tt: $args:expr),*) => {{
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                let error: isize;
                let value: usize;
                let ext: usize = $ext;
                let func: usize = $func;

                // Safety: inline assembly
                unsafe {
                    ::core::arch::asm!(
                        "ecall",
                        $(in($reg) $args),*,
                        in("a6") func, in("a7") ext,
                        lateout("a0") error, lateout("a1") value,
                    )
                };

                if error == 0 {
                    Ok(value)
                } else {
                    match error {
                        -1 => Err($crate::sbi::Error::Failed),
                        -2 => Err($crate::sbi::Error::NotSupported),
                        -3 => Err($crate::sbi::Error::InvalidParam),
                        -4 => Err($crate::sbi::Error::Denied),
                        -5 => Err($crate::sbi::Error::InvalidAddress),
                        -6 => Err($crate::sbi::Error::AlreadyAvailable),
                        -7 => Err($crate::sbi::Error::AlreadyStarted),
                        -8 => Err($crate::sbi::Error::AlreadyStopped),
                        -9 => Err($crate::sbi::Error::NoShmem),
                        code => Err($crate::sbi::Error::Other(code)),
                    }
                }
            } else {
                let _: usize = $ext;
                let _: usize = $func;
                $(let _ = $args);*;

                #[inline(always)]
                fn unimplemented() -> super::Result<usize> {
                    unimplemented!()
                }
                unimplemented()
            }
        }
    }}
}

pub(crate) use sbi_call;

bitflags! {
    #[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct Extension: u16 {
        const BASE = 1 << 0;
        const TIME = 1 << 1;
        const IPI = 1 << 2;
        const RFENCE = 1 << 3;
        const HSM = 1 << 4;
        const SRST = 1 << 5;
        const PMU = 1 << 6;
        const DBCN = 1 << 7;
        const SUSP = 1 << 8;
        const CPPC = 1 << 9;
        const NACL = 1 << 10;
        const STA = 1 << 11;
        const SSE = 1 << 11;
        const FWFT = 1 << 11;
        const DBTR = 1 << 11;
        const MPXY = 1 << 11;
    }
}

/// Probe the SBI implementation for supported extensions.
///
/// # Errors
///
/// Returns an error if one of the probing SBI calls fails.
pub fn supported_extensions() -> Result<Extension> {
    let mut supported = Extension::BASE;
    supported.set(Extension::TIME, base::probe_sbi_extension(EID_TIME)?);
    supported.set(Extension::IPI, base::probe_sbi_extension(EID_IPI)?);
    supported.set(Extension::RFENCE, base::probe_sbi_extension(EID_RFNC)?);
    supported.set(Extension::HSM, base::probe_sbi_extension(EID_HSM)?);
    supported.set(Extension::SRST, base::probe_sbi_extension(EID_SRST)?);
    supported.set(Extension::PMU, base::probe_sbi_extension(EID_PMU)?);
    supported.set(Extension::DBCN, base::probe_sbi_extension(EID_DBCN)?);
    supported.set(Extension::SUSP, base::probe_sbi_extension(EID_SUSP)?);
    supported.set(Extension::CPPC, base::probe_sbi_extension(EID_CPPC)?);
    supported.set(Extension::NACL, base::probe_sbi_extension(EID_NACL)?);
    supported.set(Extension::STA, base::probe_sbi_extension(EID_STA)?);
    supported.set(Extension::SSE, base::probe_sbi_extension(EID_SSE)?);
    supported.set(Extension::FWFT, base::probe_sbi_extension(EID_FWFT)?);
    supported.set(Extension::DBTR, base::probe_sbi_extension(EID_DBTR)?);
    supported.set(Extension::MPXY, base::probe_sbi_extension(EID_MPXY)?);
    Ok(supported)
}
