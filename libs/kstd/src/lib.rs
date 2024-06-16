#![no_std]
#![no_main]
#![feature(error_in_core, thread_local)]

pub mod arch;
mod macros;
pub mod panicking;
pub mod sync;
pub mod thread_local;
