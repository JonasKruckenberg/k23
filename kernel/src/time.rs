use core::fmt;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use core::time::Duration;

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::ZERO);

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// A measurement of a monotonically nondecreasing clock.
/// Opaque and useful only with [`Duration`].
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(Duration);

pub struct SystemTimeError(Duration);

impl Instant {
    /// Returns an instant corresponding to "now".
    pub fn now() -> Instant {
        let ticks = riscv::time::read64();
        let timebase_freq =
            crate::HART_LOCAL_MACHINE_INFO.with(|minfo| minfo.timebase_frequency) as u64;

        Instant(ticks_to_duration(ticks, timebase_freq))
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or zero duration if that instant is later than this one.
    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.checked_duration_since(earlier).unwrap_or_default()
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or zero duration if that instant is later than this one.
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.checked_duration_since(earlier).unwrap_or_default()
    }

    /// Returns the amount of time elapsed from another instant to this one,
    /// or None if that instant is later than this one.
    ///
    /// Due to [monotonicity bugs], even under correct logical ordering of the passed `Instant`s,
    /// this method can return `None`.
    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        if *self >= earlier {
            let (secs, nanos) = if self.0.subsec_nanos() >= earlier.0.subsec_nanos() {
                (
                    self.0.as_secs() - earlier.0.as_secs(),
                    self.0.subsec_nanos() - earlier.0.subsec_nanos(),
                )
            } else {
                (
                    self.0.as_secs() - earlier.0.as_secs() - 1,
                    self.0.subsec_nanos() + NANOS_PER_SEC as u32 - earlier.0.subsec_nanos(),
                )
            };

            Some(Duration::new(secs, nanos))
        } else {
            None
        }
    }

    /// Returns the amount of time elapsed since this instant.
    pub fn elapsed(&self) -> Duration {
        Instant::now() - *self
    }

    /// Returns `Some(t)` where `t` is the time `self + duration` if `t` can be represented as
    /// `Instant` or `None` otherwise.
    pub fn checked_add(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_add(duration).map(Instant)
    }

    /// Returns `Some(t)` where `t` is the time `self - duration` if `t` can be represented as
    /// `Instant` or `None` otherwise.
    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_sub(duration).map(Instant)
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

fn ticks_to_duration(ticks: u64, timebase_freq: u64) -> Duration {
    let secs = ticks / timebase_freq;
    let subsec_nanos = ((ticks % timebase_freq) * NANOS_PER_SEC / timebase_freq) as u32;
    Duration::new(secs, subsec_nanos)
}

pub fn duration_to_ticks(d: Duration, timebase_freq: u64) -> u64 {
    d.as_secs() * timebase_freq + d.subsec_nanos() as u64 * timebase_freq / NANOS_PER_SEC
}

#[cfg(test)]
mod tests {
    use crate::time::Instant;
    use core::arch::asm;
    use core::time::Duration;

    #[ktest::test]
    fn instant() {
        let start = Instant::now();

        let timebase_freq =
            crate::HART_LOCAL_MACHINE_INFO.with(|minfo| minfo.timebase_frequency) as u64;

        riscv::sbi::time::set_timer(
            riscv::time::read64()
                + crate::time::duration_to_ticks(Duration::from_secs(1), timebase_freq),
        )
        .unwrap();
        unsafe { asm!("wfi") };

        let end = Instant::now();
        let elapsed = end.duration_since(start);
        log::trace!("Time elapsed: {elapsed:?}");

        assert_eq!(elapsed.as_secs(), 1);
