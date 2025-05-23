// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std] // this is crate is fully incompatible with `std` due to clashing lang item definitions
#![cfg(target_os = "none")]
#![feature(panic_can_unwind)]
#![expect(internal_features, reason = "")]
#![feature(std_internals)]
#![feature(formatting_options)]
#![feature(never_type)]
#![feature(thread_local)]

extern crate alloc;

mod hook;
mod panic_count;

use crate::hook::{HOOK, Hook, PanicHookInfo, default_hook};
use crate::panic_count::MustAbort;
use abort::abort;
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::panic::{PanicPayload, UnwindSafe};
use core::{fmt, mem};
pub use hook::{set_hook, take_hook};

/// Determines whether the current thread is unwinding because of panic.
#[inline]
pub fn panicking() -> bool {
    !panic_count::count_is_zero()
}

/// Invokes a closure, capturing the cause of an unwinding panic if one occurs.
///
/// # Errors
///
/// If the given closure panics, the panic cause will be returned in the Err variant.
pub fn catch_unwind<F, R>(f: F) -> Result<R, Box<dyn Any + Send + 'static>>
where
    F: FnOnce() -> R + UnwindSafe,
{
    unwind2::catch_unwind(f).inspect_err(|_| {
        panic_count::decrease(); // decrease the panic count, since we caught it
    })
}

/// Resume an unwind previously caught with [`catch_unwind`].
pub fn resume_unwind(payload: Box<dyn Any + Send>) -> ! {
    debug_assert!(panic_count::increase(false).is_none());
    unwind2::with_context(|regs, pc| rust_panic(payload, regs.clone(), pc))
}

/// Begin unwinding from an externally captured set of registers (such as from a trap handler).
///
/// # Safety
///
/// This will start walking the stack and calling `Drop` implementations starting the the `pc` and
/// register set you provided. Be VERY careful that it is actually correctly captured.
pub unsafe fn begin_unwind(payload: Box<dyn Any + Send>, regs: unwind2::Registers, pc: usize) -> ! {
    debug_assert!(panic_count::increase(false).is_none());
    rust_panic(payload, regs, pc)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if let Some(must_abort) = panic_count::increase(true) {
        match must_abort {
            MustAbort::PanicInHook => {
                tracing::error!("{info}");
            }
        }

        tracing::error!("cpu panicked while processing panic. aborting.");
        abort();
    }

    let loc = info.location().unwrap(); // Currently always returns Some
    let payload = construct_panic_payload(info);

    let info = &PanicHookInfo::new(loc, payload.as_ref(), info.can_unwind());
    match *HOOK.read() {
        Hook::Default => {
            default_hook(info);
        }
        Hook::Custom(hook) => {
            hook(info);
        }
    }

    panic_count::finished_panic_hook();

    if !info.can_unwind() {
        // If a thread panics while running destructors or tries to unwind
        // through a nounwind function (e.g. extern "C") then we cannot continue
        // unwinding and have to abort immediately.
        tracing::error!("cpu caused non-unwinding panic. aborting.");
        abort();
    }

    unwind2::with_context(|regs, pc| rust_panic(payload, regs.clone(), pc))
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[unsafe(no_mangle)]
fn rust_panic(payload: Box<dyn Any + Send>, regs: unwind2::Registers, pc: usize) -> ! {
    // Safety: `begin_unwind` will either return an error or not return at all
    match unsafe { unwind2::begin_unwind_with(payload, regs, pc).unwrap_err_unchecked() } {
        unwind2::Error::EndOfStack => {
            tracing::error!(
                "unwinding completed without finding a `catch_unwind` make sure there is at least a root level catch unwind wrapping the main function. aborting."
            );
            abort();
        }
        err => {
            tracing::error!("unwinding failed with error {}. aborting.", err);
            abort()
        }
    }
}

fn construct_panic_payload(info: &core::panic::PanicInfo) -> Box<dyn Any + Send> {
    struct FormatStringPayload<'a> {
        inner: &'a core::panic::PanicMessage<'a>,
        string: Option<String>,
    }

    impl FormatStringPayload<'_> {
        fn fill(&mut self) -> &mut String {
            let inner = self.inner;
            // Lazily, the first time this gets called, run the actual string formatting.
            self.string.get_or_insert_with(|| {
                let mut s = String::new();
                let mut fmt = fmt::Formatter::new(&mut s, fmt::FormattingOptions::new());
                let _err = fmt::Display::fmt(&inner, &mut fmt);
                s
            })
        }
    }

    // Safety: TODO
    unsafe impl PanicPayload for FormatStringPayload<'_> {
        fn take_box(&mut self) -> *mut (dyn Any + Send) {
            let contents = mem::take(self.fill());
            Box::into_raw(Box::new(contents))
        }

        fn get(&mut self) -> &(dyn Any + Send) {
            self.fill()
        }
    }

    impl fmt::Display for FormatStringPayload<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            if let Some(s) = &self.string {
                f.write_str(s)
            } else {
                fmt::Display::fmt(&self.inner, f)
            }
        }
    }

    struct StaticStrPayload(&'static str);

    // Safety: TODO
    unsafe impl PanicPayload for StaticStrPayload {
        fn take_box(&mut self) -> *mut (dyn Any + Send) {
            Box::into_raw(Box::new(self.0))
        }

        fn get(&mut self) -> &(dyn Any + Send) {
            &self.0
        }

        fn as_str(&mut self) -> Option<&str> {
            Some(self.0)
        }
    }

    impl fmt::Display for StaticStrPayload {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    let msg = info.message();
    if let Some(s) = msg.as_str() {
        // Safety: take_box returns an unwrapped box
        unsafe { Box::from_raw(StaticStrPayload(s).take_box()) }
    } else {
        // Safety: take_box returns an unwrapped box
        unsafe {
            Box::from_raw(
                FormatStringPayload {
                    inner: &msg,
                    string: None,
                }
                .take_box(),
            )
        }
    }
}
