// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::sync::atomic::{AtomicUsize, Ordering};

use lock_api::GuardSend;

use crate::Backoff;

const READER: usize = 1 << 2;
const UPGRADED: usize = 1 << 1;
const WRITER: usize = 1;

pub type RwLock<T> = lock_api::RwLock<RawRwLock, T>;
pub type RwLockReadGuard<'a, T> = lock_api::RwLockReadGuard<'a, RawRwLock, T>;
pub type RwLockWriteGuard<'a, T> = lock_api::RwLockWriteGuard<'a, RawRwLock, T>;
pub type RwLockUpgradableReadGuard<'a, T> = lock_api::RwLockUpgradableReadGuard<'a, RawRwLock, T>;

pub struct RawRwLock {
    lock: AtomicUsize,
}

#[allow(clippy::undocumented_unsafe_blocks, reason = "TODO")]
unsafe impl lock_api::RawRwLock for RawRwLock {
    const INIT: Self = Self {
        lock: AtomicUsize::new(0),
    };
    type GuardMarker = GuardSend;

    fn lock_shared(&self) {
        while !self.try_lock_shared() {
            #[cfg(loom)]
            crate::loom::thread::yield_now();
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

    // fn try_lock_shared_weak(&self) -> bool {
    //     self.try_lock_shared()
    // }

    unsafe fn unlock_shared(&self) {
        debug_assert!(self.lock.load(Ordering::Relaxed) & !(WRITER | UPGRADED) > 0);
        self.lock.fetch_sub(READER, Ordering::Release);
    }

    fn lock_exclusive(&self) {
        let mut boff = Backoff::default();
        while !self.try_lock_exclusive_internal(false) {
            boff.spin();
        }
    }

    fn try_lock_exclusive(&self) -> bool {
        self.try_lock_exclusive_internal(true)
    }

    // fn try_lock_exclusive_weak(&self) -> bool {
    //     self.try_lock_exclusive_internal(false)
    // }

    unsafe fn unlock_exclusive(&self) {
        debug_assert_eq!(self.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        self.lock.fetch_and(!(WRITER | UPGRADED), Ordering::Release);
    }
}

#[allow(clippy::undocumented_unsafe_blocks, reason = "TODO")]
unsafe impl lock_api::RawRwLockUpgrade for RawRwLock {
    fn lock_upgradable(&self) {
        todo!()
    }

    fn try_lock_upgradable(&self) -> bool {
        todo!()
    }

    // fn try_lock_upgradable_weak(&self) -> bool {
    //     todo!()
    // }

    unsafe fn unlock_upgradable(&self) {
        debug_assert_eq!(
            self.lock.load(Ordering::Relaxed) & (WRITER | UPGRADED),
            UPGRADED
        );
        self.lock.fetch_sub(UPGRADED, Ordering::AcqRel);
    }

    unsafe fn upgrade(&self) {
        while self.try_upgrade_internal(false) {
            #[cfg(loom)]
            crate::loom::thread::yield_now();
            core::hint::spin_loop();
        }
    }

    unsafe fn try_upgrade(&self) -> bool {
        self.try_upgrade_internal(true)
    }

    // unsafe fn try_upgrade_weak(&self) -> bool {
    //     self.try_upgrade_internal(false)
    // }
}

#[allow(clippy::undocumented_unsafe_blocks, reason = "TODO")]
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

#[allow(clippy::undocumented_unsafe_blocks, reason = "TODO")]
unsafe impl lock_api::RawRwLockUpgradeDowngrade for RawRwLock {
    unsafe fn downgrade_upgradable(&self) {
        // Reserve the read guard for ourselves
        self.acquire_reader();

        // Safety: we just acquired the lock
        unsafe {
            <Self as lock_api::RawRwLockUpgrade>::unlock_upgradable(self);
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
    use core::fmt::Debug;
    use core::mem;
    use std::hint;
    use std::sync::mpsc::channel;

    use super::*;
    use crate::loom::sync::Arc;
    use crate::loom::thread;

    #[derive(Eq, PartialEq, Debug)]
    struct NonCopy(i32);

    #[derive(Eq, PartialEq, Debug)]
    struct NonCopyNeedsDrop(i32);

    impl Drop for NonCopyNeedsDrop {
        fn drop(&mut self) {
            hint::black_box(());
        }
    }

    #[test]
    fn test_needs_drop() {
        assert!(!mem::needs_drop::<NonCopy>());
        assert!(mem::needs_drop::<NonCopyNeedsDrop>());
    }

    #[test]
    fn smoke() {
        let l = RwLock::new(());
        drop(l.read());
        drop(l.write());
        drop((l.read(), l.read()));
        drop(l.write());
    }

    #[test]
    fn test_rw_arc() {
        let arc = Arc::new(RwLock::new(0));
        let arc2 = arc.clone();
        let (tx, rx) = channel();

        thread::spawn(move || {
            let mut lock = arc2.write();
            for _ in 0..10 {
                let tmp = *lock;
                *lock = -1;
                thread::yield_now();
                *lock = tmp + 1;
            }
            tx.send(()).unwrap();
        });

        // Readers try to catch the writer in the act
        let mut children = Vec::new();
        for _ in 0..5 {
            let arc3 = arc.clone();
            children.push(thread::spawn(move || {
                let lock = arc3.read();
                assert!(*lock >= 0);
            }));
        }

        // Wait for children to pass their asserts
        for r in children {
            assert!(r.join().is_ok());
        }

        // Wait for writer to finish
        rx.recv().unwrap();
        let lock = arc.read();
        assert_eq!(*lock, 10);
    }

    #[test]
    fn test_rwlock_unsized() {
        let rw: &RwLock<[i32]> = &RwLock::new([1, 2, 3]);
        {
            let b = &mut *rw.write();
            b[0] = 4;
            b[2] = 5;
        }
        let comp: &[i32] = &[4, 2, 5];
        assert_eq!(&*rw.read(), comp);
    }

    #[test]
    fn test_rwlock_try_write() {
        let lock = RwLock::new(0isize);
        let read_guard = lock.read();

        let write_result = lock.try_write();
        match write_result {
            None => (),
            Some(_) => assert!(
                false,
                "try_write should not succeed while read_guard is in scope"
            ),
        }
        drop(read_guard);
    }

    #[test]
    fn test_into_inner() {
        let m = RwLock::new(NonCopy(10));
        assert_eq!(m.into_inner(), NonCopy(10));
    }

    #[test]
    fn test_into_inner_drop() {
        struct Foo(Arc<AtomicUsize>);
        impl Drop for Foo {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let num_drops = Arc::new(AtomicUsize::new(0));
        let m = RwLock::new(Foo(num_drops.clone()));
        assert_eq!(num_drops.load(Ordering::SeqCst), 0);
        {
            let _inner = m.into_inner();
            assert_eq!(num_drops.load(Ordering::SeqCst), 0);
        }
        assert_eq!(num_drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_get_mut() {
        let mut m = RwLock::new(NonCopy(10));
        *m.get_mut() = NonCopy(20);
        assert_eq!(m.into_inner(), NonCopy(20));
    }

    // #[test]
    // fn test_read_guard_covariance() {
    //     fn do_stuff<'a>(_: RwLockReadGuard<'_, &'a i32>, _: &'a i32) {}
    //     let j: i32 = 5;
    //     let lock = RwLock::new(&j);
    //     {
    //         let i = 6;
    //         do_stuff(lock.read(), &i);
    //     }
    //     drop(lock);
    // }

    #[test]
    fn test_downgrade_basic() {
        let r = RwLock::new(());

        let write_guard = r.write();
        let _read_guard = RwLockWriteGuard::downgrade(write_guard);
    }

    #[test]
    fn test_downgrade_observe() {
        // Taken from the test `test_rwlock_downgrade` from:
        // https://github.com/Amanieu/parking_lot/blob/master/src/rwlock.rs

        const W: usize = 20;
        const N: usize = if cfg!(miri) { 40 } else { 100 };

        // This test spawns `W` writer threads, where each will increment a counter `N` times, ensuring
        // that the value they wrote has not changed after downgrading.

        let rw = Arc::new(RwLock::new(0));

        // Spawn the writers that will do `W * N` operations and checks.
        let handles: Vec<_> = (0..W)
            .map(|_| {
                let rw = rw.clone();
                thread::spawn(move || {
                    for _ in 0..N {
                        // Increment the counter.
                        let mut write_guard = rw.write();
                        *write_guard += 1;
                        let cur_val = *write_guard;

                        // Downgrade the lock to read mode, where the value protected cannot be modified.
                        let read_guard = RwLockWriteGuard::downgrade(write_guard);
                        assert_eq!(cur_val, *read_guard);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(*rw.read(), W * N);
    }

    #[test]
    // FIXME: On macOS we use a provenance-incorrect implementation and Miri catches that issue.
    // See <https://github.com/rust-lang/rust/issues/121950> for details.
    #[cfg_attr(all(miri, target_os = "macos"), ignore)]
    fn test_downgrade_atomic() {
        const NEW_VALUE: i32 = -1;

        // This test checks that `downgrade` is atomic, meaning as soon as a write lock has been
        // downgraded, the lock must be in read mode and no other threads can take the write lock to
        // modify the protected value.

        // `W` is the number of evil writer threads.
        const W: usize = 20;
        let rwlock = Arc::new(RwLock::new(0));

        // Spawns many evil writer threads that will try and write to the locked value before the
        // initial writer (who has the exclusive lock) can read after it downgrades.
        // If the `RwLock` behaves correctly, then the initial writer should read the value it wrote
        // itself as no other thread should be able to mutate the protected value.

        // Put the lock in write mode, causing all future threads trying to access this go to sleep.
        let mut main_write_guard = rwlock.write();

        // Spawn all of the evil writer threads. They will each increment the protected value by 1.
        let handles: Vec<_> = (0..W)
            .map(|_| {
                let rwlock = rwlock.clone();
                thread::spawn(move || {
                    // Will go to sleep since the main thread initially has the write lock.
                    let mut evil_guard = rwlock.write();
                    *evil_guard += 1;
                })
            })
            .collect();

        // Wait for a good amount of time so that evil threads go to sleep.
        // Note: this is not strictly necessary...
        let eternity = std::time::Duration::from_millis(42);
        thread::sleep(eternity);

        // Once everyone is asleep, set the value to `NEW_VALUE`.
        *main_write_guard = NEW_VALUE;

        // Atomically downgrade the write guard into a read guard.
        let main_read_guard = RwLockWriteGuard::downgrade(main_write_guard);

        // If the above is not atomic, then it would be possible for an evil thread to get in front of
        // this read and change the value to be non-negative.
        assert_eq!(*main_read_guard, NEW_VALUE, "`downgrade` was not atomic");

        // Drop the main read guard and allow the evil writer threads to start incrementing.
        drop(main_read_guard);

        for handle in handles {
            handle.join().unwrap();
        }

        let final_check = rwlock.read();
        assert_eq!(*final_check, W as i32 + NEW_VALUE);
    }
}
