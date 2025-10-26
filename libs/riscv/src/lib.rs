// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! RISC-V architecture support crate.

#![cfg_attr(not(test), no_std)]
#![allow(edition_2024_expr_fragment_specifier, reason = "vetted usage")]
#![cfg_attr(
    not(any(target_arch = "riscv32", target_arch = "riscv64")),
    allow(unused)
)]

mod error;
pub mod extensions;
pub mod hio;
pub mod interrupt;
mod macros;
pub mod register;
pub mod sbi;
pub mod semihosting;
pub mod trap;

pub use error::Error;
pub use register::*;

pub type Result<T> = core::result::Result<T, Error>;

/// Terminates the current execution with the specified exit code.
///
/// This will use the semihosting interface, if available, to exit the program. Otherwise, it will
/// enter a wfi loop.
pub fn exit(code: i32) -> ! {
    cfg_if::cfg_if! {
        if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
            semihosting::exit(code);

            // fall back to a wfi loop if exiting using semihosting failed
            // Safety: inline assembly
            unsafe {
                loop {
                    core::arch::asm!("wfi");
                }
            }
        } else {
            unimplemented!();
        }
    }
}
