use core::{any::Any, fmt};

pub use crate::panicking::update_hook;
pub use crate::panicking::{set_hook, take_hook};
pub use core::panic::Location;
pub use core::panic::{AssertUnwindSafe, RefUnwindSafe, UnwindSafe};

#[derive(Debug)]
pub struct PanicHookInfo<'a> {
    payload: &'a (dyn Any + Send),
    location: &'a Location<'a>,
    can_unwind: bool,
    force_no_backtrace: bool,
}

impl<'a> PanicHookInfo<'a> {
    #[inline]
    pub(crate) fn new(
        location: &'a Location<'a>,
        payload: &'a (dyn Any + Send),
        can_unwind: bool,
        force_no_backtrace: bool,
    ) -> Self {
        PanicHookInfo {
            payload,
            location,
            can_unwind,
            force_no_backtrace,
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
        Some(&self.location)
    }

    #[must_use]
    #[inline]
    pub fn can_unwind(&self) -> bool {
        self.can_unwind
    }

    #[doc(hidden)]
    #[inline]
    pub fn force_no_backtrace(&self) -> bool {
        self.force_no_backtrace
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
