use crate::panicking;
use alloc::{boxed::Box, string::String};
use core::{any::Any, fmt};

pub use core::panic::Location;
pub use core::panic::{AssertUnwindSafe, RefUnwindSafe, UnwindSafe};
pub use panicking::{set_hook, take_hook, update_hook};

/// # Errors
///
/// If the given closure panics, the panic cause will be returned in the Err variant.
pub fn catch_unwind<F: FnOnce() -> R + UnwindSafe, R>(
    f: F,
) -> Result<R, Box<dyn Any + Send + 'static>> {
    unsafe { panicking::r#try(f) }
}

pub fn resume_unwind(payload: Box<dyn Any + Send>) -> ! {
    panicking::rust_panic_without_hook(payload)
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
