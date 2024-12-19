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
mod error;
pub mod machine_info;
mod start;
mod thread_local;
pub mod time;
mod tracing_subscriber;
pub mod vm;
// mod tests;

pub use error::Error;
pub use start::{BOOT_INFO, HART_LOCAL_MACHINE_INFO, MACHINE_INFO};
pub type Result<T> = core::result::Result<T, Error>;

/// The log level for the kernel
pub const LOG_LEVEL: log::Level = log::Level::Trace;
/// The size of the stack in pages
pub const STACK_SIZE_PAGES: u32 = 256;
/// The size of the trap handler stack in pages
pub const TRAP_STACK_SIZE_PAGES: usize = 16;
/// The initial size of the kernel heap in pages.
///
/// This initial size should be small enough so the loaders less sophisticated allocator can
/// doesn't cause startup slowdown & inefficient mapping, but large enough so we can bootstrap
/// our own virtual memory subsystem. At that point we are no longer reliant on this initial heap
/// size and can dynamically grow the heap as needed.
pub const HEAP_SIZE_PAGES: u32 = 16; // 32 MiB
