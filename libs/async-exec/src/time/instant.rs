// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::time::{Clock, NANOS_PER_SEC, Ticks};
use core::fmt;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use core::time::Duration;

/// A measurement of a monotonically nondecreasing clock.
/// Opaque and useful only with [`Duration`].
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(pub(super) Duration);

impl Instant {
    pub const ZERO: Self = Self(Duration::ZERO);

    /// Returns an instant corresponding to "now" in the global timer.
    pub fn now(clock: &Clock) -> Self {
        clock.now()
    }

    /// Create a new `Instant` from the given raw [`Ticks`] using the global timer and clock.
    /// Panics if no global timer is configured.
    pub fn from_ticks(clock: &Clock, ticks: Ticks) -> Self {
        let duration = clock.ticks_to_duration(ticks);
        Self(duration)
    }

    /// Create a timestamp in the "very-far-future", panicking if no global timer is configured.
    ///
    /// TODO elaborate
    pub fn far_future(clock: &Clock) -> Self {
        // Returns an instant roughly 30 years from now.
        // This is used instead of `Duration::MAX` because conversion to ticks might cause an overflow
        // but doing checked or saturating conversions in those functions is too expensive.
        clock.now() + Duration::from_secs(86400 * 365 * 30)
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or zero duration if that instant is later than this one.
    pub fn duration_since(&self, earlier: Self) -> Duration {
        self.checked_duration_since(earlier).unwrap_or_default()
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or zero duration if that instant is later than this one.
    pub fn saturating_duration_since(&self, earlier: Self) -> Duration {
        self.checked_duration_since(earlier).unwrap_or_default()
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or None if that instant is later than this one.
    ///
    /// Due to [monotonicity bugs], even under correct logical ordering of the passed `Instant`s,
    /// this method can return `None`.
    pub fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
        if *self >= earlier {
            let (secs, nanos) = if self.0.subsec_nanos() >= earlier.0.subsec_nanos() {
                (
                    self.0.as_secs() - earlier.0.as_secs(),
                    self.0.subsec_nanos() - earlier.0.subsec_nanos(),
                )
            } else {
                (
                    self.0.as_secs() - earlier.0.as_secs() - 1,
                    self.0.subsec_nanos()
                        // Safety: always fits
                        + unsafe { u32::try_from(NANOS_PER_SEC).unwrap_unchecked() }
                        - earlier.0.subsec_nanos(),
                )
            };

            Some(Duration::new(secs, nanos))
        } else {
            None
        }
    }

    /// Returns `Some(t)` where `t` is the time `self + duration` if `t` can be represented as
    /// `Instant` or `None` otherwise.
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration).map(Self)
    }

    /// Returns `Some(t)` where `t` is the time `self - duration` if `t` can be represented as
    /// `Instant` or `None` otherwise.
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(duration).map(Self)
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;

    /// # Panics
    ///
    /// This function may panic if the resulting point in time cannot be represented by the
    /// underlying data structure. See [`Instant::checked_add`] for a version without panic.
    fn add(self, other: Duration) -> Instant {
        self.checked_add(other)
            .expect("overflow when adding duration to instant")
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, other: Duration) {
        *self = *self + other;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;

    fn sub(self, other: Duration) -> Instant {
        self.checked_sub(other)
            .expect("overflow when subtracting duration from instant")
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, other: Duration) {
        *self = *self - other;
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;

    /// Returns the amount of time elapsed from another instant to this one,
    /// or zero duration if that instant is later than this one.
    ///
    /// # Panics
    ///
    /// Previous Rust versions panicked when `other` was later than `self`. Currently this
    /// method saturates. Future versions may reintroduce the panic in some circumstances.
    /// See [Monotonicity].
    ///
    /// [Monotonicity]: Instant#monotonicity
    fn sub(self, other: Instant) -> Duration {
        self.duration_since(other)
    }
}

impl fmt::Debug for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
