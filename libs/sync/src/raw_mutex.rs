// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::Backoff;
use core::sync::atomic::{AtomicBool, Ordering};
use lock_api::GuardSend;

/// Low-level mutual exclusion lock.
///
/// This type of lock allow at most one reader *or* writer at any point in time.
pub struct RawMutex {
    lock: AtomicBool,
}

// Safety: unsafe trait
unsafe impl lock_api::RawMutex for RawMutex {
    #[expect(clippy::declare_interior_mutable_const, reason = "")] // TODO figure out
    const INIT: Self = Self {
        lock: AtomicBool::new(false),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        let mut boff = Backoff::default();
        while self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.is_locked() {
                boff.spin();
            }
        }
    }

    fn try_lock(&self) -> bool {
        self.lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    unsafe fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }
}
