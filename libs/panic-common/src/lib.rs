#![no_std]
#![allow(internal_features)]
#![feature(fmt_internals, std_internals, panic_can_unwind)]

extern crate alloc;

pub mod hook;

use alloc::boxed::Box;
use alloc::string::String;
use cfg_if::cfg_if;
use core::any::Any;
use core::panic::{Location, PanicPayload};
use core::{fmt, mem};

pub fn abort() -> ! {
    cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            loop {}
        }
    }
}

pub fn exit(code: i32) -> ! {
    cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::exit(code);
        } else {
            loop {}
        }
    }
}

#[inline]
pub fn with_panic_info<F, R>(info: &core::panic::PanicInfo, f: F) -> R
where
    F: FnOnce(&mut dyn PanicPayload, &Location<'_>, bool) -> R,
{
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
                let mut fmt = fmt::Formatter::new(&mut s);
                let _err = fmt::Display::fmt(&inner, &mut fmt);
                s
            })
        }
    }

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
    let loc = info.location().unwrap(); // Currently, this is always `Some`.
    if let Some(s) = msg.as_str() {
        f(&mut StaticStrPayload(s), loc, info.can_unwind())
    } else {
        f(
            &mut FormatStringPayload {
                inner: &msg,
                string: None,
            },
            loc,
            info.can_unwind(),
        )
    }
}

#[derive(Debug)]
pub struct PanicHookInfo<'a> {
    payload: &'a (dyn Any + Send),
    location: &'a Location<'a>,
    can_unwind: bool,
}

impl<'a> PanicHookInfo<'a> {
    #[inline]
    pub fn new(
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
    pub fn payload(&self) -> &'a (dyn Any + Send) {
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
    pub fn location(&self) -> &Location<'_> {
        // NOTE: If this is changed to sometimes return None,
        // deal with that case in std::panicking::default_hook and core::panicking::panic_fmt.
        self.location
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
