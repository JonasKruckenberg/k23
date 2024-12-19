//! Support for time-related functionality. This module mirrors Rusts `std::time` module.

use crate::{arch, MACHINE_INFO};
use core::fmt;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use core::sync::atomic::{AtomicPtr, Ordering};
use core::time::Duration;

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::ZERO);

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// A measurement of a monotonically nondecreasing clock.
/// Opaque and useful only with [`Duration`].
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(Duration);

/// A measurement of the system clock
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SystemTime(Duration);

/// An error returned from the `duration_since` and `elapsed` methods on
/// `SystemTime`, used to learn how far in the opposite direction a system time
/// lies.
#[derive(Clone, Debug)]
pub struct SystemTimeError(Duration);

impl Instant {
    /// Returns an instant corresponding to "now".
    pub fn now() -> Instant {
        let ticks = arch::time::read64();

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

impl SystemTime {
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    pub fn now() -> Self {
        // Only device supported right now is "google,goldfish-rtc"
        // https://android.googlesource.com/platform/external/qemu/+/master/docs/GOLDFISH-VIRTUAL-HARDWARE.TXT

        let rtc = MACHINE_INFO.get().unwrap().rtc.as_ref().unwrap();
        let time_ns = unsafe {
            let time_low = AtomicPtr::new(rtc.start.as_raw() as *mut u32);
            let time_high = AtomicPtr::new(rtc.start.add(0x04).as_raw() as *mut u32);

            let low = time_low.load(Ordering::Relaxed).read_volatile();
            let high = time_high.load(Ordering::Relaxed).read_volatile();

            ((high as u64) << 32) | low as u64
        };

        SystemTime(Duration::new(
            time_ns / NANOS_PER_SEC,
            (time_ns % NANOS_PER_SEC) as u32,
        ))
    }
    pub fn duration_since(&self, earlier: SystemTime) -> Result<Duration, SystemTimeError> {
        if self >= &earlier {
            Ok(self.0 - earlier.0)
        } else {
            Err(SystemTimeError(earlier.0 - self.0))
        }
    }
    pub fn elapsed(&self) -> Result<Duration, SystemTimeError> {
        SystemTime::now().duration_since(*self)
    }
    pub fn checked_add(&self, duration: Duration) -> Option<SystemTime> {
        self.0.checked_add(duration).map(SystemTime)
    }
    pub fn checked_sub(&self, duration: Duration) -> Option<SystemTime> {
        self.0.checked_sub(duration).map(SystemTime)
    }
}

impl Add<Duration> for SystemTime {
    type Output = SystemTime;

    /// # Panics
    ///
    /// This function may panic if the resulting point in time cannot be represented by the
    /// underlying data structure. See [`SystemTime::checked_add`] for a version without panic.
    fn add(self, dur: Duration) -> SystemTime {
        self.checked_add(dur)
            .expect("overflow when adding duration to instant")
    }
}

impl AddAssign<Duration> for SystemTime {
    fn add_assign(&mut self, other: Duration) {
        *self = *self + other;
    }
}

impl Sub<Duration> for SystemTime {
    type Output = SystemTime;

    fn sub(self, dur: Duration) -> SystemTime {
        self.checked_sub(dur)
            .expect("overflow when subtracting duration from instant")
    }
}

impl SubAssign<Duration> for SystemTime {
    fn sub_assign(&mut self, other: Duration) {
        *self = *self - other;
    }
}

impl fmt::Debug for SystemTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl SystemTimeError {
    /// Returns the positive duration which represents how far forward the
    /// second system time was from the first.
    ///
    /// A `SystemTimeError` is returned from the [`SystemTime::duration_since`]
    /// and [`SystemTime::elapsed`] methods whenever the second system time
    /// represents a point later in time than the `self` of the method call.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::thread::sleep;
    /// use std::time::{Duration, SystemTime};
    ///
    /// let sys_time = SystemTime::now();
    /// sleep(Duration::from_secs(1));
    /// let new_sys_time = SystemTime::now();
    /// match sys_time.duration_since(new_sys_time) {
    ///     Ok(_) => {}
    ///     Err(e) => println!("SystemTimeError difference: {:?}", e.duration()),
    /// }
    /// ```
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.0
    }
}

impl core::error::Error for SystemTimeError {}

impl fmt::Display for SystemTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "second time provided was later than self")
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

/// low-level sleep primitive, will sleep the calling hart for at least the specified duration
///
/// # Safety
///
/// This function is very low level and will block the calling hart until a timer interrupt is received.
/// No checking is performed however if the timer interrupt is the correct one.
pub unsafe fn sleep(duration: Duration) {
    let timebase_freq =
        crate::HART_LOCAL_MACHINE_INFO.with(|minfo| minfo.timebase_frequency) as u64;

    riscv::sbi::time::set_timer(riscv::time::read64() + duration_to_ticks(duration, timebase_freq))
        .unwrap();

    arch::wait_for_interrupt();
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;

    #[ktest::test]
    fn instant() {
        let start = Instant::now();

        unsafe {
            sleep(Duration::from_secs(1));
        }

        let end = Instant::now();
        let elapsed = end.duration_since(start);
        log::trace!("Time elapsed: {elapsed:?}");

        assert_eq!(elapsed.as_secs(), 1);
    }
}
