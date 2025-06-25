// new file

#![no_std]
#![cfg_attr(feature = "nightly", feature(naked_functions))]

pub mod error;
pub mod interrupt;
pub mod io;
// pub mod register;
pub mod trap;

pub use error::Error;
pub use interrupt::{disable as interrupt_disable, enable as interrupt_enable};
pub use trap::{Exception, Interrupt, Trap};