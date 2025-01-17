// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::sync::atomic::{AtomicUsize, Ordering};
use lock_api::RawRwLockUpgrade as _;

/// Low-level reader-writer lock.
///
/// This type of lock allows a number of readers or at most one writer at any point in time.
pub struct RawRwLock {
    lock: AtomicUsize,
}

const READER: usize = 1 << 2;
const UPGRADED: usize = 1 << 1;
const WRITER: usize = 1;

impl RawRwLock {
    fn acquire_reader(&self) -> usize {
        // An arbitrary cap that allows us to catch overflows long before they happen
        const MAX_READERS: usize = usize::MAX / READER / 2;

        let value = self.lock.fetch_add(READER, Ordering::Acquire);

        if value > MAX_READERS * READER {
            self.lock.fetch_sub(READER, Ordering::Relaxed);
            panic!("Too many lock readers, cannot safely proceed");
        } else {
            value
        }
    }

    fn try_lock_exclusive_internal(&self, strong: bool) -> bool {
        compare_exchange(
            &self.lock,
            0,
            WRITER,
            Ordering::Acquire,
            Ordering::Relaxed,
            strong,
        )
        .is_ok()
    }

    fn try_upgrade_internal(&self, strong: bool) -> bool {
        compare_exchange(
            &self.lock,
            UPGRADED,
            WRITER,
            Ordering::Acquire,
            Ordering::Relaxed,
            strong,
        )
        .is_ok()
    }
}

// Safety: unsafe trait
unsafe impl lock_api::RawRwLock for RawRwLock {
    #[expect(clippy::declare_interior_mutable_const, reason = "")]
    const INIT: Self = Self {
        lock: AtomicUsize::new(0),
    };

    type GuardMarker = lock_api::GuardSend;

    fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed) != 0
    }

    fn lock_shared(&self) {
        while !self.try_lock_shared() {
            core::hint::spin_loop();
        }
    }

    fn try_lock_shared(&self) -> bool {
        let value = self.acquire_reader();

        // We check the UPGRADED bit here so that new readers are prevented when an UPGRADED lock is held.
        // This helps reduce writer starvation.
        if value & (WRITER | UPGRADED) != 0 {
            // Lock is taken, undo.
            self.lock.fetch_sub(READER, Ordering::Release);
            false
        } else {
            true
        }
    }

    unsafe fn unlock_shared(&self) {
        debug_assert!(self.lock.load(Ordering::Relaxed) & !(WRITER | UPGRADED) > 0);
        self.lock.fetch_sub(READER, Ordering::Release);
    }

    fn lock_exclusive(&self) {
        while !self.try_lock_exclusive_internal(false) {
            core::hint::spin_loop();
        }
    }

    fn try_lock_exclusive(&self) -> bool {
        self.try_lock_exclusive_internal(true)
    }

    unsafe fn unlock_exclusive(&self) {
        debug_assert_eq!(self.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        self.lock.fetch_and(!(WRITER | UPGRADED), Ordering::Release);
    }
}

// Safety: unsafe trait
unsafe impl lock_api::RawRwLockUpgrade for RawRwLock {
    fn lock_upgradable(&self) {
        while !self.try_lock_upgradable() {
            core::hint::spin_loop();
        }
    }

    fn try_lock_upgradable(&self) -> bool {
        if self.lock.fetch_or(UPGRADED, Ordering::Acquire) & (WRITER | UPGRADED) == 0 {
            true
        } else {
            // We can't unflip the UPGRADED bit back just yet as there is another upgradeable or write lock.
            // When they unlock, they will clear the bit.
            false
        }
    }

    unsafe fn unlock_upgradable(&self) {
        debug_assert_eq!(
            self.lock.load(Ordering::Relaxed) & (WRITER | UPGRADED),
            UPGRADED
        );
        self.lock.fetch_sub(UPGRADED, Ordering::AcqRel);
    }

    unsafe fn upgrade(&self) {
        while !self.try_upgrade_internal(false) {
            core::hint::spin_loop();
        }
    }

    unsafe fn try_upgrade(&self) -> bool {
        self.try_upgrade_internal(true)
    }
}

// Safety: unsafe trait
unsafe impl lock_api::RawRwLockDowngrade for RawRwLock {
    unsafe fn downgrade(&self) {
        // Reserve the read guard for ourselves
        self.acquire_reader();

        debug_assert_eq!(self.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        self.lock.fetch_and(!(WRITER | UPGRADED), Ordering::Release);
    }
}

// Safety: unsafe trait
unsafe impl lock_api::RawRwLockUpgradeDowngrade for RawRwLock {
    unsafe fn downgrade_upgradable(&self) {
        // Reserve the read guard for ourselves
        self.acquire_reader();

        // Safety: we just acquired the lock
        unsafe {
            self.unlock_upgradable();
        }
    }

    unsafe fn downgrade_to_upgradable(&self) {
        debug_assert_eq!(
            self.lock.load(Ordering::Acquire) & (WRITER | UPGRADED),
            WRITER
        );

        // Reserve the read guard for ourselves
        self.lock.store(UPGRADED, Ordering::Release);

        debug_assert_eq!(self.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        self.lock.fetch_and(!(WRITER | UPGRADED), Ordering::Release);
    }
}

fn compare_exchange(
    atomic: &AtomicUsize,
    current: usize,
    new: usize,
    success: Ordering,
    failure: Ordering,
    strong: bool,
) -> Result<usize, usize> {
    if strong {
        atomic.compare_exchange(current, new, success, failure)
    } else {
        atomic.compare_exchange_weak(current, new, success, failure)
    }
}
