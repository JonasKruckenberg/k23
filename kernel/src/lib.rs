// You might wonder: Why is this also a library?
//
// The reason is that other crates in this workspace (currently only `ktest`) depend on the runtime
// services provided by this crate. I.e. the panic handler, global allocator, trap handler etc.
// To avoid code duplication, the `ktest` crate just depend on the kernel, overriding the `kmain`
// function with its own test runner.
#![no_std]
#![no_main]
#![allow(internal_features)]
#![feature(used_with_arg, naked_functions, thread_local, allocator_api)]
#![feature(panic_can_unwind, std_internals, fmt_internals)]

extern crate alloc;

mod allocator;
pub mod arch;
pub mod kconfig;
mod start;
// mod tests;
