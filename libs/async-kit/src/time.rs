// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod clock;
mod instant;
mod sleep;
mod timeout;
mod timer;

pub const NANOS_PER_SEC: u64 = 1_000_000_000;

use core::fmt;
use core::fmt::Formatter;
use core::time::Duration;

pub use clock::{Clock, Ticks};
pub use instant::Instant;
pub use sleep::{sleep, sleep_until, Sleep};
pub use timeout::{timeout, Elapsed, Timeout};
pub use timer::{global::set_global_timer, global::AlreadyInitialized, Deadline, Timer};

#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TimeError {
    NoGlobalTimer,
    DurationTooLong {
        /// The duration that was requested for a [`Sleep`] or [`Timeout`]
        /// future.
        ///
        /// [`Timeout`]: crate::time::Timeout
        requested: Duration,
        /// The [maximum duration][max] supported by this [`Timer`] instance.
        ///
        /// [max]: Timer::max_duration
        max: Duration,
    },
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TimeError::NoGlobalTimer => f.write_str("no global timer available. Tip: You can configure the global timer with `async_kit::time::set_global_timer`"),
            TimeError::DurationTooLong { requested, max } => write!(f, "duration too long: {requested:?}. Maximum duration {max:?}"),
        }
    }
}

impl core::error::Error for TimeError {}
