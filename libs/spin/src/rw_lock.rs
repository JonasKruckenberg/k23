// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Implementation based on the `RwSpinLock` from facebook's folly: https://github.com/facebook/folly/blob/main/folly/synchronization/RWSpinLock.h

use core::fmt::Formatter;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::{fmt, hint};

use crate::Backoff;

const READER: usize = 1 << 2;
const UPGRADED: usize = 1 << 1;
const WRITER: usize = 1;

pub type RwLock<T> = lock_api::RwLock<RawRwLock, T>;
pub type RwLockWriteGuard<'a, T> = lock_api::RwLockWriteGuard<'a, RawRwLock, T>;
pub type RwLockReadGuard<'a, T> = lock_api::RwLockReadGuard<'a, RawRwLock, T>;
pub type RwLockUpgradableReadGuard<'a, T> = lock_api::RwLockUpgradableReadGuard<'a, RawRwLock, T>;
pub type MappedRwLockWriteGuard<'a, T> = lock_api::MappedRwLockWriteGuard<'a, RawRwLock, T>;
pub type MappedRwLockReadGuard<'a, T> = lock_api::MappedRwLockReadGuard<'a, RawRwLock, T>;

pub struct RawRwLock {
    lock: AtomicUsize,
}

impl fmt::Debug for RawRwLock {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let v = self.lock.load(Ordering::Relaxed);
        f.debug_struct("RawRwLock")
            .field("writer", &(v & WRITER != 0))
            .field("upgraded", &(v & UPGRADED != 0))
            .field("readers", &((v & !(READER - 1)) >> READER))
            .finish()
    }
}

impl RawRwLock {
    #[inline(always)]
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

    #[inline(always)]
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

// Safety: TODO
unsafe impl lock_api::RawRwLock for RawRwLock {
    const INIT: Self = Self {
        lock: AtomicUsize::new(0),
    };
    type GuardMarker = lock_api::GuardSend;

    fn lock_shared(&self) {
        let mut boff = Backoff::new();
        while !self.try_lock_shared() {
            hint::cold_path();
            boff.spin();
        }
    }

    // Try to get reader permission on the lock. This can fail if we
    // find out someone is a writer or upgrader.
    // Setting the UPGRADED bit would allow a writer-to-be to indicate
    // its intention to write and block any new readers while waiting
    // for existing readers to finish and release their read locks. This
    // helps avoid starving writers (promoted from upgraders).
    fn try_lock_shared(&self) -> bool {
        // fetch_add is considerably (100%) faster than compare_exchange,
        // so here we are optimizing for the common (lock success) case.
        let prev = self.lock.fetch_add(READER, Ordering::Acquire);

        if prev & (WRITER | UPGRADED) != 0 {
            hint::cold_path();
            self.lock.fetch_sub(READER, Ordering::Acquire);
            false
        } else {
            true
        }
    }

    unsafe fn unlock_shared(&self) {
        self.lock.fetch_sub(READER, Ordering::Release);
    }

    fn lock_exclusive(&self) {
        let mut boff = Backoff::new();
        while !self.try_lock_exclusive_internal(false) {
            hint::cold_path();
            boff.spin();
        }
    }

    #[inline]
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

// Safety: TODO
unsafe impl lock_api::RawRwLockUpgrade for RawRwLock {
    fn lock_upgradable(&self) {
        let mut boff = Backoff::new();
        while !self.try_lock_upgradable() {
            hint::cold_path();
            boff.spin();
        }
    }

    #[inline]
    fn try_lock_upgradable(&self) -> bool {
        let prev = self.lock.fetch_or(UPGRADED, Ordering::Acquire);

        // When this comparison fails, we cannot flip the UPGRADED bit back,
        // as in this case there is either another upgrade lock or a write lock.
        // In either case, they will clear the bit when they unlock
        (prev & (WRITER | UPGRADED)) == 0
    }

    #[inline]
    unsafe fn unlock_upgradable(&self) {
        self.lock.fetch_sub(UPGRADED, Ordering::AcqRel);
    }

    unsafe fn upgrade(&self) {
        let mut boff = Backoff::new();
        while !self.try_upgrade_internal(false) {
            hint::cold_path();
            boff.spin();
        }
    }

    #[inline]
    unsafe fn try_upgrade(&self) -> bool {
        self.try_upgrade_internal(true)
    }
}

// Safety: TODO
unsafe impl lock_api::RawRwLockDowngrade for RawRwLock {
    unsafe fn downgrade(&self) {
        use lock_api::RawRwLock;

        self.lock.fetch_add(READER, Ordering::Acquire);

        // Safety: TODO
        unsafe {
            self.unlock_exclusive();
        }
    }
}

// Safety: TODO
unsafe impl lock_api::RawRwLockUpgradeDowngrade for RawRwLock {
    unsafe fn downgrade_upgradable(&self) {
        // Increase the number of READERs and drop the UPGRADED bit
        self.lock.fetch_add(READER - UPGRADED, Ordering::AcqRel);
    }

    unsafe fn downgrade_to_upgradable(&self) {
        self.lock.fetch_or(UPGRADED, Ordering::Acquire);
        self.lock.fetch_sub(WRITER, Ordering::Release);
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

#[cfg(test)]
mod tests {
    use lock_api::{
        RawRwLock as _, RawRwLockDowngrade, RawRwLockUpgrade, RawRwLockUpgradeDowngrade,
    };

    use super::*;
    use crate::loom;
    use crate::loom::thread;

    const MAX_READERS: usize = 50;

    /// Assert that a write lock cannot be obtained while read locks are held
    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn writer_waits_for_readers() {
        let l = RawRwLock::INIT;

        for _ in 0..MAX_READERS {
            assert!(l.try_lock_shared());
            assert!(!l.try_lock_exclusive());
        }

        for _ in 0..MAX_READERS {
            assert!(!l.try_lock_exclusive());
            unsafe {
                l.unlock_shared();
            }
        }

        assert!(l.try_lock_exclusive());
    }

    /// Assert that read locks cannot be obtained while a write lock is held
    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn readers_wait_for_writer() {
        let l = RawRwLock::INIT;

        assert!(l.try_lock_exclusive());
        assert!(l.is_locked_exclusive());
        assert!(l.is_locked());

        for _ in 0..MAX_READERS {
            assert!(!l.try_lock_shared());
        }

        unsafe {
            l.unlock_exclusive();
        }

        for _ in 0..MAX_READERS {
            assert!(l.try_lock_shared());
            assert!(l.is_locked());
        }
    }

    /// Assert that a write lock cannot be obtained while another write lock is held
    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn writer_waits_for_writer() {
        let l = RawRwLock::INIT;

        assert!(l.try_lock_exclusive());
        assert!(l.is_locked_exclusive());
        assert!(l.is_locked());

        // we cannot obtain a second write lock
        assert!(!l.try_lock_exclusive());

        unsafe {
            l.unlock_exclusive();
        }

        // after unlocking we can
        assert!(l.try_lock_exclusive());
        assert!(!l.try_lock_exclusive());
    }

    /// Assert that downgrading an exclusive lock will allow readers & writers to make progress
    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn downgrade() {
        let l = RawRwLock::INIT;

        assert!(l.try_lock_exclusive());
        assert!(l.is_locked_exclusive());
        assert!(l.is_locked());

        for _ in 0..MAX_READERS {
            assert!(!l.try_lock_shared());
        }

        unsafe {
            l.downgrade();
        }

        for _ in 0..MAX_READERS {
            assert!(l.try_lock_shared());
            assert!(l.is_locked());
        }
    }

    /// Assert that an upgradable lock cannot be obtained when a write lock is held
    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn upgradeable_wait_writer() {
        let l = RawRwLock::INIT;

        assert!(l.try_lock_exclusive());
        assert!(l.is_locked_exclusive());
        assert!(l.is_locked());

        // cannot lock while write lock is held
        assert!(!l.try_lock_upgradable());

        unsafe { l.unlock_exclusive() };

        // after write lock is released we can
        assert!(l.try_lock_upgradable());
    }

    /// Assert that read locks cannot be obtained while an upgradable lock is held
    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn readers_wait_for_upgradable_unlock_upgradable() {
        let l = RawRwLock::INIT;

        assert!(l.try_lock_upgradable());
        assert!(l.is_locked());

        for _ in 0..MAX_READERS {
            assert!(!l.try_lock_shared());
        }

        // unlock the upgradable lock
        unsafe {
            l.unlock_upgradable();
        }

        for _ in 0..MAX_READERS {
            assert!(l.try_lock_shared());
            assert!(l.is_locked());
        }
    }

    /// Assert that downgrading an upgradable lock allows readers to make progress
    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn downgrade_upgradable() {
        let l = RawRwLock::INIT;

        assert!(l.try_lock_upgradable());
        assert!(l.is_locked());

        for _ in 0..MAX_READERS {
            assert!(!l.try_lock_shared());
        }

        // downgrade the upgradable lock into a shared lock
        unsafe {
            l.downgrade_upgradable();
        }

        for _ in 0..MAX_READERS {
            assert!(l.try_lock_shared());
            assert!(l.is_locked());
        }
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn upgradable_does_not_wait_for_readers() {
        let l = RawRwLock::INIT;

        for _ in 0..MAX_READERS {
            assert!(l.try_lock_shared());
        }

        // even though we have read locks, we can still obtain an upgradable lock ...
        assert!(l.try_lock_upgradable());

        // ... but we cannot upgrade it into a write lock
        assert!(!unsafe { l.try_upgrade() });

        for _ in 0..MAX_READERS {
            unsafe { l.unlock_shared() }
        }

        // after all the read have unlocked we can upgrade!
        assert!(unsafe { l.try_upgrade() });
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn lock_unlock_tests() {
        let l = RawRwLock::INIT;

        assert!(l.try_lock_upgradable());
        assert!(!l.try_lock_shared());
        assert!(!l.try_lock_exclusive());
        assert!(!l.try_lock_upgradable());

        unsafe {
            l.unlock_upgradable();
        }

        assert!(!l.is_locked());

        assert!(l.try_lock_shared());
        assert!(!l.try_lock_exclusive());
        assert!(l.try_lock_upgradable());

        unsafe {
            l.unlock_upgradable();
        }
        unsafe {
            l.unlock_shared();
        }

        assert!(!l.is_locked());

        assert!(l.try_lock_exclusive());
        assert!(!l.try_lock_upgradable());

        unsafe {
            l.downgrade_to_upgradable();
        }

        assert!(!l.try_lock_shared());

        unsafe { l.downgrade_upgradable() };
        unsafe {
            l.unlock_shared();
        }

        assert_eq!(0, l.lock.load(Ordering::Relaxed), "{l:?}");
    }

    /// Number of cycles to repeat concurrency tests for, but loom and miri are really slow
    /// and should probably catch any bugs after much fewer iterations anyway
    const CYCLES: usize = if cfg!(loom) | cfg!(miri) { 100 } else { 500 };

    #[test]
    fn concurrent_tests() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: RawRwLock = RawRwLock::INIT;
                static ref READS: AtomicUsize = AtomicUsize::new(0);
                static ref WRITES: AtomicUsize = AtomicUsize::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..loom::MAX_THREADS - 1 {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        if rand::random_bool(0.1) {
                            L.lock_exclusive();
                            unsafe { L.unlock_exclusive() };
                            WRITES.fetch_add(1, Ordering::AcqRel);
                        } else {
                            L.lock_shared();
                            unsafe { L.unlock_shared() };
                            READS.fetch_add(1, Ordering::AcqRel);
                        }

                        #[cfg(loom)]
                        thread::yield_now();
                    }
                }));
            }

            for t in threads {
                t.join().unwrap();
            }

            println!(
                "READS: {}; WRITES: {};",
                READS.load(Ordering::Acquire),
                WRITES.load(Ordering::Acquire),
            );
        })
    }

    #[test]
    fn concurrent_holder_test() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: RawRwLock = RawRwLock::INIT;
                static ref READS: AtomicUsize = AtomicUsize::new(0);
                static ref WRITES: AtomicUsize = AtomicUsize::new(0);
                static ref UPGRADES: AtomicUsize = AtomicUsize::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..loom::MAX_THREADS - 1 {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let r = rand::random::<u8>();
                        if r < 3 {
                            // starts from write lock
                            L.lock_exclusive();
                            unsafe { L.downgrade_to_upgradable() };
                            unsafe { L.downgrade_upgradable() };
                            unsafe { L.unlock_shared() };
                            WRITES.fetch_add(1, Ordering::AcqRel);
                        } else if r < 6 {
                            // starts from upgrade lock
                            L.lock_upgradable();

                            if r < 4 {
                                unsafe { L.upgrade() };
                                unsafe { L.unlock_exclusive() };
                            } else {
                                unsafe { L.downgrade_upgradable() };
                                unsafe { L.unlock_shared() };
                            }

                            UPGRADES.fetch_add(1, Ordering::AcqRel);
                        } else {
                            // starts from read lock
                            L.lock_shared();
                            unsafe { L.unlock_shared() };

                            READS.fetch_add(1, Ordering::AcqRel);
                        }

                        #[cfg(loom)]
                        thread::yield_now();
                    }
                }));
            }

            for t in threads {
                t.join().unwrap();
            }

            println!(
                "READS: {}; WRITES: {}; UPGRADES: {}",
                READS.load(Ordering::Acquire),
                WRITES.load(Ordering::Acquire),
                UPGRADES.load(Ordering::Acquire)
            );
        })
    }
}
