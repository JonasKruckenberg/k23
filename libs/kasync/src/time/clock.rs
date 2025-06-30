// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::time::max_duration;
use core::fmt;
use core::time::Duration;

pub struct Clock {
    name: &'static str,
    tick_duration: Duration,
    clock: RawClock,
}

/// A virtual
///
/// # Safety
///
///
/// These functions must all be thread-safe
pub struct RawClockVTable {
    clone: unsafe fn(*const ()) -> RawClock,
    now: unsafe fn(*const ()) -> u64,
    schedule_wakeup: unsafe fn(*const (), at: u64),
    drop: unsafe fn(*const ()),
}

pub struct RawClock {
    data: *const (),
    vtable: &'static RawClockVTable,
}

// === impl Clock ===

impl Unpin for Clock {}

// Safety: As part of the safety contract for RawClockVTable, the caller promised RawClock is Send
// therefore Clock is Send too
unsafe impl Send for Clock {}
// Safety: As part of the safety contract for RawClockVTable, the caller promised RawClock is Sync
// therefore Clock is Sync too
unsafe impl Sync for Clock {}

impl Clock {
    #[inline]
    #[must_use]
    pub const unsafe fn from_raw(tick_duration: Duration, clock: RawClock) -> Clock {
        Self {
            clock,
            tick_duration,
            name: "<unnamed mystery clock>",
        }
    }

    #[inline]
    #[must_use]
    pub const unsafe fn new(
        tick_duration: Duration,
        data: *const (),
        vtable: &'static RawClockVTable,
    ) -> Clock {
        // Safety: ensured by caller
        unsafe { Self::from_raw(tick_duration, RawClock { data, vtable }) }
    }

    #[must_use]
    pub const fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Returns this `Clock`'s name, if it was given one using the [`Clock::named`]
    /// method.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the [`Duration`] of one tick of this clock.
    #[must_use]
    pub const fn tick_duration(&self) -> Duration {
        self.tick_duration
    }

    /// Returns the maximum duration of this clock.
    #[must_use]
    pub fn max_duration(&self) -> Duration {
        max_duration(self.tick_duration())
    }

    /// Gets the `data` pointer used to create this `Clock`.
    #[inline]
    #[must_use]
    pub fn data(&self) -> *const () {
        self.clock.data
    }

    /// Gets the `vtable` pointer used to create this `Clock`.
    #[inline]
    #[must_use]
    pub fn vtable(&self) -> &'static RawClockVTable {
        self.clock.vtable
    }

    #[inline]
    pub fn now(&self) -> u64 {
        unsafe { (self.clock.vtable.now)(self.clock.data) }
    }

    #[inline]
    pub fn schedule_wakeup(&self, at: u64) {
        unsafe { (self.clock.vtable.schedule_wakeup)(self.clock.data, at) };
    }
}

impl Clone for Clock {
    #[inline]
    fn clone(&self) -> Self {
        Clock {
            // SAFETY: This is safe because `Waker::from_raw` is the only way
            // to initialize `clone` and `data` requiring the user to acknowledge
            // that the contract of [`RawWaker`] is upheld.
            clock: unsafe { (self.clock.vtable.clone)(self.clock.data) },
            tick_duration: self.tick_duration,
            name: self.name,
        }
    }
}

impl Drop for Clock {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: This is safe because `Waker::from_raw` is the only way
        // to initialize `drop` and `data` requiring the user to acknowledge
        // that the contract of `RawWaker` is upheld.
        unsafe { (self.clock.vtable.drop)(self.clock.data) }
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let vtable_ptr = self.clock.vtable as *const RawClockVTable;
        f.debug_struct("Waker")
            .field("name", &self.name)
            .field("tick_duration", &self.tick_duration)
            .field("data", &self.clock.data)
            .field("vtable", &vtable_ptr)
            .finish()
    }
}

impl fmt::Display for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}, {:?} precision", self.name, self.tick_duration)
    }
}

// === impl RawClock ===

impl RawClock {
    #[inline]
    #[must_use]
    pub const fn new(data: *const (), vtable: &'static RawClockVTable) -> RawClock {
        Self { data, vtable }
    }
}

// === impl RawClockVTable ===

impl RawClockVTable {
    pub const fn new(
        clone: unsafe fn(*const ()) -> RawClock,
        now: unsafe fn(*const ()) -> u64,
        schedule_wakeup: unsafe fn(*const (), at: u64),
        drop: unsafe fn(*const ()),
    ) -> Self {
        Self {
            clone,
            now,
            schedule_wakeup,
            drop,
        }
    }
}
