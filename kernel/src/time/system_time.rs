// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use core::time::Duration;

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::ZERO);

/// A measurement of the system clock
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SystemTime(Duration);

/// An error returned from the `duration_since` and `elapsed` methods on
/// `SystemTime`, used to learn how far in the opposite direction a system time
/// lies.
#[derive(Clone, Debug)]
pub struct SystemTimeError(Duration);

impl SystemTime {
    #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
    pub fn now() -> Self {
        // Only device supported right now is "google,goldfish-rtc"
        // https://android.googlesource.com/platform/external/qemu/+/master/docs/GOLDFISH-VIRTUAL-HARDWARE.TXT

        // machine_info().
        // let rtc = MACHINE_INFO
        //     .get()
        //     .unwrap()
        //     .mmio_devices
        //     .iter()
        //     .find(|region| region.compatible.contains(&"google,goldfish-rtc"))
        //     .unwrap();

        // Safety: MMIO device access
        // let time_ns = unsafe {
        //     assert!(rtc.regions[0].start.is_aligned_to(4));
        //
        //     #[expect(clippy::cast_ptr_alignment, reason = "checked above")]
        //     let time_low = AtomicPtr::new(rtc.regions[0].start.as_mut_ptr().cast::<u32>());
        //     #[expect(clippy::cast_ptr_alignment, reason = "checked above")]
        //     let time_high = AtomicPtr::new(
        //         rtc.regions[0]
        //             .start
        //             .checked_add(0x04)
        //             .unwrap()
        //             .as_mut_ptr()
        //             .cast::<u32>(),
        //     );
        //
        //     let low = time_low.load(Ordering::Relaxed).read_volatile();
        //     let high = time_high.load(Ordering::Relaxed).read_volatile();
        //
        //     (u64::from(high) << 32_i32) | u64::from(low)
        // };
        //
        // SystemTime(Duration::new(
        //     time_ns / NANOS_PER_SEC,
        //     (time_ns % NANOS_PER_SEC) as u32,
        // ))

        todo!()
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
    #[expect(unused, reason = "")]
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
