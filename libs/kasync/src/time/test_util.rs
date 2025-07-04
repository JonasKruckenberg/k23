// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::Arc;
use crate::loom::sync::Mutex as StdMutex;
use crate::time::{Clock, RawClock, RawClockVTable};
use core::ptr::NonNull;
use core::time::Duration;
use std::time::Instant as StdInstant;

pub struct MockClock {
    time_anchor: StdInstant,
    now: StdMutex<StdInstant>,
}

impl MockClock {
    pub fn new(tick_duration: Duration) -> Clock {
        let now = StdInstant::now();

        let ptr = Arc::into_raw(Arc::new(MockClock {
            time_anchor: now,
            now: StdMutex::new(now),
        }));

        unsafe { Clock::new(tick_duration, ptr.cast(), &Self::VTABLE).named("mock test clock") }
    }

    pub fn new_1us() -> Clock {
        Self::new(Duration::from_micros(1))
    }

    pub fn advance(&self, tick_duration: Duration) {
        *self.now.lock().unwrap() += tick_duration;
    }

    // === RawClock ===

    const VTABLE: RawClockVTable = RawClockVTable::new(
        Self::clone_raw,
        Self::now_raw,
        Self::schedule_wakeup_raw,
        Self::drop_raw,
    );

    unsafe fn clone_raw(ptr: *const ()) -> RawClock {
        tracing::trace!(
            clock.addr = ?ptr,
            "StdClock::clone_raw"
        );

        unsafe { Arc::increment_strong_count(ptr.cast::<MockClock>()) }
        RawClock::new(ptr, &Self::VTABLE)
    }

    unsafe fn now_raw(ptr: *const ()) -> u64 {
        let ptr = ptr.cast::<MockClock>();
        tracing::trace!(
            clock.addr = ?ptr,
            "StdClock::now_raw"
        );

        let me = unsafe { NonNull::new_unchecked(ptr.cast_mut()) };
        let me = unsafe { me.as_ref() };

        let elapsed = me.now.lock().unwrap().duration_since(me.time_anchor);

        elapsed.as_micros() as u64
    }

    unsafe fn schedule_wakeup_raw(ptr: *const (), _at: u64) {
        let ptr = ptr.cast::<MockClock>();
        tracing::trace!(
            clock.addr = ?ptr,
            "StdClock::schedule_wakeup_raw"
        );

        // we do nothing here, the clock has to manually advanced anyway
    }

    unsafe fn drop_raw(ptr: *const ()) {
        let ptr = ptr.cast::<MockClock>();
        tracing::trace!(
            clock.addr = ?ptr,
            "StdClock::drop_raw"
        );
        drop(unsafe { Arc::from_raw(ptr) });
    }
}
