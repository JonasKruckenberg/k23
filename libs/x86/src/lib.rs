// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![cfg_attr(feature = "nightly", feature(naked_functions))]

pub mod error;
pub mod interrupt;
pub mod io;
// pub mod register;
pub mod serial;
pub mod trap;

pub use error::Error;
pub use interrupt::{disable as interrupt_disable, enable as interrupt_enable};
pub use trap::{Exception, Interrupt, Trap};
