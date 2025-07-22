// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::time::Duration;

use crate::time::max_duration;

pub struct Clock {
    name: &'static str,
    tick_duration: Duration,
    clock: RawClock,
}

/// A virtual function pointer table (vtable) that specifies the behavior
/// of a [`RawClock`].
///
/// The pointer passed to all functions inside the vtable is the `data` pointer
/// from the enclosing [`RawClock`] object.
///
/// The functions inside this struct are only intended to be called on the `data`
/// pointer of a properly constructed [`RawClock`] object from inside the
/// [`RawClock`] implementation. Calling one of the contained functions using
/// any other `data` pointer will cause undefined behavior.
///
/// # Thread safety
///
/// All vtable functions must be thread-safe (even though [`RawClock`] is
/// <code>\![Send] + \![Sync]</code>). This is because [`Clock`] is <code>[Send] + [Sync]</code>,
/// and it *will* be moved to arbitrary threads or invoked by `&` reference. For example,
/// this means that if the `clone` and `drop` functions manage a reference count,
/// they must do so atomically.
#[derive(Copy, Clone, Debug)]
pub struct RawClockVTable {
    clone: unsafe fn(*const ()) -> RawClock,
    now: unsafe fn(*const ()) -> u64,
    schedule_wakeup: unsafe fn(*const (), at: u64),
    drop: unsafe fn(*const ()),
}

#[derive(Debug)]
pub struct RawClock {
    /// The `data` pointer can be used to store arbitrary data as required by the clock implementation.
    data: *const (),
    /// Virtual function pointer table that customizes the behavior of this clock.
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
    /// Creates a new `Clock` from the provided `tick_duration`, `data` pointer and `vtable`.
    ///
    /// The `tick_duration` is the `Duration` of time represented by a single `u64` tick in this clock.
    /// This is in effect the precision of the clock and should be set to the precision of the underlying
    /// hardware timer.
    ///
    /// The `data` pointer can be used to store arbitrary data as required by the clock implementation.
    /// This could be e.g. a type-erased pointer to an `Arc` that holds private implementation-specific state.
    /// The value of this pointer will get passed to all functions that are part
    /// of the `vtable` as the first parameter.
    ///
    /// It is important to consider that the `data` pointer must point to a
    /// thread safe type such as an `Arc`.
    ///
    /// The `vtable` customizes the behavior of a `Clock`. For each operation
    /// on the `Clock`, the associated function in the `vtable` will be called.
    ///
    /// # Safety
    ///
    /// The behavior of the returned `Clock` is undefined if the contract defined
    /// in [`RawClockVTable`]'s documentation is not upheld.
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

    /// Creates a new `Clock` from a [`RawClock`].
    ///
    /// # Safety
    ///
    /// The behavior of the returned `Waker` is undefined if the contract defined
    /// in [`RawClock`]'s and [`RawClockVTable`]'s documentation is not upheld.
    #[inline]
    #[must_use]
    pub const unsafe fn from_raw(tick_duration: Duration, clock: RawClock) -> Clock {
        Self {
            clock,
            tick_duration,
            name: "<unnamed mystery clock>",
        }
    }

    /// Add an arbitrary user-defined name to this `Clock`.
    ///
    /// This is generally used to describe the hardware time source used by the
    /// `now()` function for this `Clock`.
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
        // Safety: This is safe because `Clock::from_raw` is the only way
        // to initialize `vtable` and `data` requiring the user to acknowledge
        // that the contract of `RawClock` is upheld.
        unsafe { (self.clock.vtable.now)(self.clock.data) }
    }

    #[inline]
    pub fn schedule_wakeup(&self, at: u64) {
        // Safety: see Clock::now
        unsafe { (self.clock.vtable.schedule_wakeup)(self.clock.data, at) };
    }
}

impl Clone for Clock {
    #[inline]
    fn clone(&self) -> Self {
        Clock {
            // SAFETY: see Clock::now
            clock: unsafe { (self.clock.vtable.clone)(self.clock.data) },
            tick_duration: self.tick_duration,
            name: self.name,
        }
    }
}

impl Drop for Clock {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: see Clock::now
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
    /// Creates a new `Clock` from the provided `data` pointer and `vtable`.
    ///
    /// The `data` pointer can be used to store arbitrary data as required by the clock implementation.
    /// his could be e.g. a type-erased pointer to an `Arc` that holds private implementation-specific state.
    /// The value of this pointer will get passed to all functions that are part
    /// of the `vtable` as the first parameter.
    ///
    /// It is important to consider that the `data` pointer must point to a
    /// thread safe type such as an `Arc`.
    ///
    /// The `vtable` customizes the behavior of a `Clock`. For each operation
    /// on the `Clock`, the associated function in the `vtable` will be called.
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
