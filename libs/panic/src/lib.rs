//! Panic infrastructure for k23.
//!
//! This crate provides a few things:
//! 1. It defines the `#[panic_handler]` required for `core` panics.
//! 2. It provided panic hooks for consumers to define custom panic logging (`set_hook`, `take_hook`, `update_hook`).
//! 3. It provides a `catch_unwind` function for catching unwinding panics.
//!
//! # Cargo Features
//!
//! - `abort` - Immediately aborts the execution on panic.
//! - `unwind` - Unwinds the stack on panic. This is a requirement for `catch_unwind` to work properly.
//! - `backtrace` - Prints a backtrace in the default panic hook.
#![no_std]
#![allow(internal_features)]
#![feature(std_internals, panic_can_unwind, fmt_internals)]
#![cfg_attr(
    feature = "unwind",
    feature(rustc_attrs, core_intrinsics, thread_local)
)]

extern crate alloc;

mod panicking;

cfg_if::cfg_if! {
    if #[cfg(all(feature = "unwind", feature = "abort"))] {
        compile_error!("only one of the `unwind` or `abort` features can be enabled");
    } else if #[cfg(feature = "unwind")] {
        mod unwind;
        use unwind as r#impl;
    } else if #[cfg(feature = "abort")] {
        mod abort;
        use abort as r#impl;
    }
}

cfg_if::cfg_if! {
    if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
        use riscv as arch;
    } else {
        compile_error!("unsupported target architecture");
    }
}

use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::fmt;
use core::panic::{Location, UnwindSafe};

pub use panicking::{set_hook, take_hook, update_hook};

/// Invokes a closure, capturing the cause of an unwinding panic if one occurs.
///
/// # Errors
///
/// If the given closure panics, the panic cause will be returned in the Err variant.
pub fn catch_unwind<F: FnOnce() -> R + UnwindSafe, R>(
    f: F,
) -> Result<R, Box<dyn Any + Send + 'static>> {
    unsafe { r#impl::r#try(f) }
}

/// Triggers a panic, bypassing the panic hook.
pub fn resume_unwind(payload: Box<dyn Any + Send>) -> ! {
    panicking::rust_panic_without_hook(payload)
}

/// Determines whether the current thread is unwinding because of panic.
#[inline]
pub fn panicking() -> bool {
    !r#impl::panic_count::count_is_zero()
}

#[derive(Debug)]
pub struct PanicHookInfo<'a> {
    payload: &'a (dyn Any + Send),
    location: &'a Location<'a>,
    can_unwind: bool,
}

impl<'a> PanicHookInfo<'a> {
    #[inline]
    pub(crate) fn new(
        location: &'a Location<'a>,
        payload: &'a (dyn Any + Send),
        can_unwind: bool,
    ) -> Self {
        PanicHookInfo {
            payload,
            location,
            can_unwind,
        }
    }

    #[must_use]
    #[inline]
    pub fn payload(&self) -> &(dyn Any + Send) {
        self.payload
    }

    #[must_use]
    #[inline]
    pub fn payload_as_str(&self) -> Option<&str> {
        if let Some(s) = self.payload.downcast_ref::<&str>() {
            Some(s)
        } else if let Some(s) = self.payload.downcast_ref::<String>() {
            Some(s)
        } else {
            None
        }
    }

    #[must_use]
    #[inline]
    pub fn location(&self) -> Option<&Location<'_>> {
        // NOTE: If this is changed to sometimes return None,
        // deal with that case in std::panicking::default_hook and core::panicking::panic_fmt.
        Some(self.location)
    }

    #[must_use]
    #[inline]
    pub fn can_unwind(&self) -> bool {
        self.can_unwind
    }

    // #[doc(hidden)]
    // #[inline]
    // pub fn force_no_backtrace(&self) -> bool {
    //     self.force_no_backtrace
    // }
}

impl fmt::Display for PanicHookInfo<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("panicked at ")?;
        self.location.fmt(formatter)?;
        if let Some(payload) = self.payload_as_str() {
            formatter.write_str(":\n")?;
            formatter.write_str(payload)?;
        }
        Ok(())
    }
}
