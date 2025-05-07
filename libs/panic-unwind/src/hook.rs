// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::string::String;
use core::any::Any;
use core::panic::Location;
use core::{fmt, mem};
use spin::RwLock;

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

    /// Returns the payload associated with the panic.
    ///
    /// This will commonly, but not always, be a `&'static str` or [`String`].
    ///
    /// A invocation of the `panic!()` macro in Rust 2021 or later will always result in a
    /// panic payload of type `&'static str` or `String`.
    ///
    /// Only an invocation of [`panic_any`]
    /// (or, in Rust 2018 and earlier, `panic!(x)` where `x` is something other than a string)
    /// can result in a panic payload other than a `&'static str` or `String`.
    ///
    /// [`String`]: ../../std/string/struct.String.html
    ///
    /// # Examples
    ///
    /// ```should_panic
    /// use std::panic;
    ///
    /// panic::set_hook(Box::new(|panic_info| {
    ///     if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
    ///         println!("panic occurred: {s:?}");
    ///     } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
    ///         println!("panic occurred: {s:?}");
    ///     } else {
    ///         println!("panic occurred");
    ///     }
    /// }));
    ///
    /// panic!("Normal panic");
    /// ```
    #[must_use]
    #[inline]
    pub fn payload(&self) -> &(dyn Any + Send) {
        self.payload
    }

    /// Returns the payload associated with the panic, if it is a string.
    ///
    /// This returns the payload if it is of type `&'static str` or `String`.
    ///
    /// A invocation of the `panic!()` macro in Rust 2021 or later will always result in a
    /// panic payload where `payload_as_str` returns `Some`.
    ///
    /// Only an invocation of [`panic_any`]
    /// (or, in Rust 2018 and earlier, `panic!(x)` where `x` is something other than a string)
    /// can result in a panic payload where `payload_as_str` returns `None`.
    ///
    /// # Example
    ///
    /// ```should_panic
    /// #![feature(panic_payload_as_str)]
    ///
    /// std::panic::set_hook(Box::new(|panic_info| {
    ///     if let Some(s) = panic_info.payload_as_str() {
    ///         println!("panic occurred: {s:?}");
    ///     } else {
    ///         println!("panic occurred");
    ///     }
    /// }));
    ///
    /// panic!("Normal panic");
    /// ```
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

    /// Returns information about the location from which the panic originated,
    /// if available.
    ///
    /// This method will currently always return [`Some`], but this may change
    /// in future versions.
    ///
    /// # Examples
    ///
    /// ```should_panic
    /// use std::panic;
    ///
    /// panic::set_hook(Box::new(|panic_info| {
    ///     if let Some(location) = panic_info.location() {
    ///         println!("panic occurred in file '{}' at line {}",
    ///             location.file(),
    ///             location.line(),
    ///         );
    ///     } else {
    ///         println!("panic occurred but can't get location information...");
    ///     }
    /// }));
    ///
    /// panic!("Normal panic");
    /// ```
    #[must_use]
    #[inline]
    pub fn location(&self) -> &Location<'_> {
        self.location
    }

    /// Returns whether the panic handler is allowed to unwind the stack from
    /// the point where the panic occurred.
    ///
    /// This is true for most kinds of panics with the exception of panics
    /// caused by trying to unwind out of a `Drop` implementation or a function
    /// whose ABI does not support unwinding.
    ///
    /// It is safe for a panic handler to unwind even when this function returns
    /// false, however this will simply cause the panic handler to be called
    /// again.
    #[must_use]
    #[inline]
    pub fn can_unwind(&self) -> bool {
        self.can_unwind
    }
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

pub(crate) static HOOK: RwLock<Hook> = RwLock::new(Hook::Default);

#[derive(Default)]
pub(crate) enum Hook {
    #[default]
    Default,
    Custom(fn(&PanicHookInfo<'_>)),
}

impl Hook {
    #[inline]
    fn into_fn(self) -> fn(&PanicHookInfo<'_>) {
        match self {
            Hook::Default => default_hook,
            Hook::Custom(hook) => hook,
        }
    }
}

/// Registers a custom panic hook, replacing the previously registered hook.
///
/// # Panics
///
/// Panics if called from an already panicking CPU
pub fn set_hook(hook: fn(&PanicHookInfo<'_>)) {
    assert!(
        !crate::panicking(),
        "cannot modify the panic hook from a panicking CPU"
    );

    let new = Hook::Custom(hook);
    let mut hook = HOOK.write();
    let _ = mem::replace(&mut *hook, new);
}

/// Unregisters the current panic hook and returns it, registering the default hook
/// in its place.
///
/// # Panics
///
/// Panics if called from an already panicking CPU
pub fn take_hook() -> fn(&PanicHookInfo<'_>) {
    assert!(
        !crate::panicking(),
        "cannot modify the panic hook from a panicking CPU"
    );

    let mut hook = HOOK.write();
    let old_hook = mem::take(&mut *hook);
    drop(hook);

    old_hook.into_fn()
}

/// The default panic handler.
pub(crate) fn default_hook(info: &PanicHookInfo<'_>) {
    tracing::error!("{info}");
}
