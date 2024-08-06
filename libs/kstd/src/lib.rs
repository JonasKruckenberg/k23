#![no_std]
// panicking & unwinding
#![feature(
    naked_functions,
    lang_items,
    panic_info_message,
    std_internals,
    used_with_arg,
    panic_can_unwind,
    fmt_internals,
    core_intrinsics,
    rustc_attrs
)]
#![allow(internal_features)]
// thread_local
#![feature(thread_local)]
#![allow(clippy::module_name_repetitions)]

extern crate alloc;

mod macros;
mod panicking;

// Architecture specific code
pub mod arch;

// Syncronization primitives
pub mod sync;

// DWARF-based stack unwinding
#[cfg(feature = "panic-unwind")]
pub mod unwinding;

// Public-facing panic API
pub mod panic;

// Thread-local storage
pub mod thread_local;
