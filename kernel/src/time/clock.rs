// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::time::{Instant, NANOS_PER_SEC};
use core::time::Duration;

/// [`Clock`] ticks are always counted by a 64-bit unsigned integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ticks(pub u64);

/// A hardware clock definition.
///
/// A `Clock` consists of a function that returns the hardware clock's current
/// timestamp in [`Ticks`] (`now()`), and a [`Duration`] that defines the amount
/// of time represented by a single tick of the clock.
#[derive(Debug, Clone)]
pub struct Clock {
    now: fn() -> Ticks,
    tick_duration: Duration,
    name: &'static str,
}

impl Clock {
    #[must_use]
    pub const fn new(tick_duration: Duration, now: fn() -> Ticks) -> Self {
        Self {
            now,
            tick_duration,
            name: "<unnamed mystery clock>",
        }
    }

    /// Add an arbitrary user-defined name to this `Clock`.
    ///
    /// This is generally used to describe the hardware time source used by the
    /// `now()` function for this `Clock`.
    #[must_use]
    pub const fn named(self, name: &'static str) -> Self {
        Self { name, ..self }
    }

    /// Returns the current `now` timestamp, in [`Ticks`] of this clock's base
    /// tick duration.
    #[must_use]
    pub(crate) fn now_ticks(&self) -> Ticks {
        (self.now)()
    }

    /// Returns the [`Duration`] of one tick of this clock.
    #[must_use]
    pub fn tick_duration(&self) -> Duration {
        self.tick_duration
    }

    /// Returns an [`Instant`] representing the current timestamp according to
    /// this [`Clock`].
    #[must_use]
    pub fn now(&self) -> Instant {
        let now = self.now_ticks();
        Instant(self.ticks_to_duration(now))
    }

    /// Returns the maximum duration of this clock.
    #[must_use]
    pub fn max_duration(&self) -> Duration {
        max_duration(self.tick_duration())
    }

    /// Returns this `Clock`'s name, if it was given one using the [`Clock::named`]
    /// method.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn ticks_to_duration(&self, ticks: Ticks) -> Duration {
        // Multiply nanoseconds as u64, because it cannot overflow that way.
        let total_nanos = self.tick_duration.subsec_nanos() as u64 * ticks.0;
        let extra_secs = total_nanos / (NANOS_PER_SEC);
        let nanos = (total_nanos % (NANOS_PER_SEC)) as u32;
        let Some(secs) = self.tick_duration.as_secs().checked_mul(ticks.0) else {
            panic!(
                "ticks_to_dur({:?}, {ticks:?}): multiplying tick \
            duration seconds by ticks would overflow",
                self.tick_duration
            );
        };
        let Some(secs) = secs.checked_add(extra_secs) else {
            panic!("ticks_to_dur({:?}, {ticks:?}): extra seconds from nanos ({extra_secs}s) would overflow total seconds", self.tick_duration)
        };
        debug_assert!(nanos < NANOS_PER_SEC as u32);
        Duration::new(secs, nanos)
    }

    pub fn duration_to_ticks(&self, duration: Duration) -> Result<Ticks, super::Error> {
        let raw: u64 = (duration.as_nanos() / self.tick_duration.as_nanos())
            .try_into()
            .map_err(|_| super::Error::DurationTooLong {
                requested: duration,
                max: max_duration(self.tick_duration),
            })?;

        Ok(Ticks(raw))
    }
}

#[inline]
#[must_use]
pub(super) fn max_duration(tick_duration: Duration) -> Duration {
    tick_duration.saturating_mul(u32::MAX)
}
