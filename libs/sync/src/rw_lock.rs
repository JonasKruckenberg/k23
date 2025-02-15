// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::{loom_const_fn, AtomicUsize, Ordering, UnsafeCell};
use crate::Backoff;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use core::{fmt, mem};

const READER: usize = 1 << 2;
const UPGRADED: usize = 1 << 1;
const WRITER: usize = 1;

pub struct RwLock<T: ?Sized> {
    lock: AtomicUsize,
    data: UnsafeCell<T>,
}

/// RAII structure used to release the shared read access of a lock when
/// dropped.
#[clippy::has_significant_drop]
#[must_use = "if unused the RwLock will immediately unlock"]
pub struct RwLockReadGuard<'a, T: ?Sized + 'a> {
    // NB: we use a pointer instead of `&'a T` to avoid `noalias` violations, because a
    // `RwLockReadGuard` argument doesn't hold immutability for its whole scope, only until it drops.
    // `NonNull` is also covariant over `T`, just like we would have with `&T`. `NonNull`
    // is preferable over `const* T` to allow for niche optimization.
    _data: NonNull<T>,
    rwlock: &'a RwLock<T>,
}

/// RAII structure used to release the exclusive write access of a lock when
/// dropped.
#[clippy::has_significant_drop]
#[must_use = "if unused the RwLock will immediately unlock"]
pub struct RwLockWriteGuard<'a, T: ?Sized> {
    rwlock: &'a RwLock<T>,
}

/// RAII structure used to release the upgradable read access of a lock when
/// dropped.
#[clippy::has_significant_drop]
#[must_use = "if unused the RwLock will immediately unlock"]
pub struct RwLockUpgradableReadGuard<'a, T: ?Sized + 'a> {
    // NB: we use a pointer instead of `&'a T` to avoid `noalias` violations, because a
    // `RwLockReadGuard` argument doesn't hold immutability for its whole scope, only until it drops.
    // `NonNull` is also covariant over `T`, just like we would have with `&T`. `NonNull`
    // is preferable over `const* T` to allow for niche optimization.
    data: NonNull<T>,
    rwlock: &'a RwLock<T>,
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    loom_const_fn! {
        /// Creates a new instance of an `RwLock<T>` which is unlocked.
        #[inline]
        #[expect(tail_expr_drop_order, reason = "")]
        pub fn new(val: T) -> RwLock<T> {
            RwLock {
                data: UnsafeCell::new(val),
                lock: AtomicUsize::new(0),
            }
        }
    }

    /// Consumes this `RwLock`, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> RwLock<T> {
    /// Creates a new `RwLockReadGuard` without checking if the lock is held.
    ///
    /// # Safety
    ///
    /// This method must only be called if the thread logically holds a read lock.
    ///
    /// This function does not increment the read count of the lock. Calling this function when a
    /// guard has already been produced is undefined behaviour unless the guard was forgotten
    /// with `mem::forget`.
    #[inline]
    pub unsafe fn make_read_guard_unchecked(&self) -> RwLockReadGuard<'_, T> {
        RwLockReadGuard {
            _data: self.data.with_mut(|data| {
                // Safety: ensured by caller
                unsafe { NonNull::new_unchecked(data) }
            }),
            rwlock: self,
        }
    }

    /// Creates a new `RwLockReadGuard` without checking if the lock is held.
    ///
    /// # Safety
    ///
    /// This method must only be called if the thread logically holds a write lock.
    ///
    /// Calling this function when a guard has already been produced is undefined behaviour unless
    /// the guard was forgotten with `mem::forget`.
    #[inline]
    pub unsafe fn make_write_guard_unchecked(&self) -> RwLockWriteGuard<'_, T> {
        RwLockWriteGuard { rwlock: self }
    }

    /// Locks this `RwLock` with shared read access, blocking the current thread
    /// until it can be acquired.
    ///
    /// The calling thread will be blocked until there are no more writers which
    /// hold the lock. There may be other readers currently inside the lock when
    /// this method returns.
    ///
    /// Note that attempts to recursively acquire a read lock on a `RwLock` when
    /// the current thread already holds one may result in a deadlock.
    ///
    /// Returns an RAII guard which will release this thread's shared access
    /// once it is dropped.
    #[inline]
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.lock_shared();

        // SAFETY: The lock is held, as required.
        unsafe { self.make_read_guard_unchecked() }
    }

    /// Attempts to acquire this `RwLock` with shared read access.
    ///
    /// If the access could not be granted at this time, then `None` is returned.
    /// Otherwise, an RAII guard is returned which will release the shared access
    /// when it is dropped.
    ///
    /// This function does not block.
    #[inline]
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        if self.try_lock_shared() {
            // SAFETY: The lock is held, as required.
            Some(unsafe { self.make_read_guard_unchecked() })
        } else {
            None
        }
    }

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

    /// Locks this `RwLock` with exclusive write access, blocking the current
    /// thread until it can be acquired.
    ///
    /// This function will not return while other writers or other readers
    /// currently have access to the lock.
    ///
    /// Returns an RAII guard which will drop the write access of this `RwLock`
    /// when dropped.
    #[inline]
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        self.lock_exclusive();
        // SAFETY: The lock is held, as required.
        unsafe { self.make_write_guard_unchecked() }
    }

    /// Attempts to lock this `RwLock` with exclusive write access.
    ///
    /// If the lock could not be acquired at this time, then `None` is returned.
    /// Otherwise, an RAII guard is returned which will release the lock when
    /// it is dropped.
    ///
    /// This function does not block.
    #[inline]
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        if self.try_lock_exclusive() {
            // SAFETY: The lock is held, as required.
            Some(unsafe { self.make_write_guard_unchecked() })
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// Since this call borrows the `RwLock` mutably, no actual locking needs to
    /// take place---the mutable borrow statically guarantees no locks exist.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.with_mut(|data| {
            // Safety: We hold a mutable reference to the RwLock so getting a mutable reference to the
            // data is safe
            unsafe { &mut *data }
        })
    }

    /// Checks whether this `RwLock` is currently locked in any way.
    #[inline]
    pub fn is_locked(&self) -> bool {
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

    fn lock_exclusive(&self) {
        let mut boff = Backoff::default();
        while !self.try_lock_exclusive_internal(false) {
            boff.spin();
        }
    }

    fn try_lock_exclusive(&self) -> bool {
        self.try_lock_exclusive_internal(true)
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

    unsafe fn unlock_shared(&self) {
        debug_assert!(self.lock.load(Ordering::Relaxed) & !(WRITER | UPGRADED) > 0);
        self.lock.fetch_sub(READER, Ordering::Release);
    }

    unsafe fn unlock_exclusive(&self) {
        debug_assert_eq!(self.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        self.lock.fetch_and(!(WRITER | UPGRADED), Ordering::Release);
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

    unsafe fn downgrade_upgradable(&self) {
        // Reserve the read guard for ourselves
        self.acquire_reader();

        // Safety: we just acquired the lock
        unsafe {
            self.unlock_upgradable();
        }
    }
}

impl<T: Default> Default for RwLock<T> {
    #[inline]
    fn default() -> RwLock<T> {
        RwLock::new(Default::default())
    }
}

impl<T> From<T> for RwLock<T> {
    #[inline]
    fn from(t: T) -> RwLock<T> {
        RwLock::new(t)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("RwLock");
        match self.try_read() {
            Some(guard) => d.field("data", &&*guard),
            None => {
                // Additional format_args! here is to remove quotes around <locked> in debug output.
                d.field("data", &format_args!("<locked>"))
            }
        };
        d.finish()
    }
}

impl<T: ?Sized> !Send for RwLockReadGuard<'_, T> {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<T: Sync + ?Sized> Sync for RwLockReadGuard<'_, T> {}

impl<'a, T: ?Sized + 'a> RwLockReadGuard<'a, T> {
    /// Returns a reference to the original reader-writer lock object.
    pub fn rwlock(s: &Self) -> &'a RwLock<T> {
        s.rwlock
    }
}

impl<'a, T: ?Sized + 'a> Deref for RwLockReadGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.rwlock.data.with(|data| {
            // Safety: RwLockReadGuard always holds a read lock, so obtaining an immutable reference
            // is safe
            unsafe { &*data }
        })
    }
}

impl<'a, T: ?Sized + 'a> Drop for RwLockReadGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: An RwLockReadGuard always holds a shared lock.
        unsafe {
            self.rwlock.unlock_shared();
        }
    }
}

impl<'a, T: fmt::Debug + ?Sized + 'a> fmt::Debug for RwLockReadGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: fmt::Display + ?Sized + 'a> fmt::Display for RwLockReadGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized> !Send for RwLockWriteGuard<'_, T> {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<T: Sync + ?Sized> Sync for RwLockWriteGuard<'_, T> {}

impl<'a, T: ?Sized + 'a> RwLockWriteGuard<'a, T> {
    /// Returns a reference to the original reader-writer lock object.
    pub fn rwlock(s: &Self) -> &'a RwLock<T> {
        s.rwlock
    }

    /// Atomically downgrades a write lock into a read lock without allowing any
    /// writers to take exclusive access of the lock in the meantime.
    ///
    /// Note that if there are any writers currently waiting to take the lock
    /// then other readers may not be able to acquire the lock even if it was
    /// downgraded.
    pub fn downgrade(s: Self) -> RwLockReadGuard<'a, T> {
        let rwlock = s.rwlock;

        // Reserve the read guard for ourselves
        rwlock.acquire_reader();

        debug_assert_eq!(rwlock.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        rwlock
            .lock
            .fetch_and(!(WRITER | UPGRADED), Ordering::Release);

        mem::forget(s);
        RwLockReadGuard {
            _data: rwlock.data.with_mut(|data| {
                // Safety: RwLockWriteGuard holds mutable access to the data
                unsafe { NonNull::new_unchecked(data) }
            }),
            rwlock,
        }
    }

    /// Atomically downgrades a write lock into an upgradable read lock without allowing any
    /// writers to take exclusive access of the lock in the meantime.
    ///
    /// Note that if there are any writers currently waiting to take the lock
    /// then other readers may not be able to acquire the lock even if it was
    /// downgraded.
    pub fn downgrade_to_upgradable(s: Self) -> RwLockUpgradableReadGuard<'a, T> {
        let rwlock = s.rwlock;

        debug_assert_eq!(
            rwlock.lock.load(Ordering::Acquire) & (WRITER | UPGRADED),
            WRITER
        );

        // Reserve the read guard for ourselves
        rwlock.lock.store(UPGRADED, Ordering::Release);

        debug_assert_eq!(rwlock.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        rwlock
            .lock
            .fetch_and(!(WRITER | UPGRADED), Ordering::Release);

        mem::forget(s);
        RwLockUpgradableReadGuard {
            data: rwlock.data.with_mut(|data| {
                // Safety: RwLockWriteGuard holds mutable access to the data
                unsafe { NonNull::new_unchecked(data) }
            }),
            rwlock,
        }
    }
}

impl<'a, T: ?Sized + 'a> Deref for RwLockWriteGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.rwlock.data.with(|data| {
            // Safety: RwLockWriteGuard always holds a read lock, so obtaining an immutable reference
            // is safe
            unsafe { &*data }
        })
    }
}

impl<'a, T: ?Sized + 'a> DerefMut for RwLockWriteGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.rwlock.data.with_mut(|data| {
            // Safety: RwLockWriteGuard always holds a write lock, so obtaining a mutable reference
            // is safe
            unsafe { &mut *data }
        })
    }
}

impl<'a, T: ?Sized + 'a> Drop for RwLockWriteGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: An RwLockWriteGuard always holds an exclusive lock.
        unsafe {
            self.rwlock.unlock_exclusive();
        }
    }
}

impl<'a, T: fmt::Debug + ?Sized + 'a> fmt::Debug for RwLockWriteGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: fmt::Display + ?Sized + 'a> fmt::Display for RwLockWriteGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized> !Send for RwLockUpgradableReadGuard<'_, T> {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<'a, T: ?Sized + Sync + 'a> Sync for RwLockUpgradableReadGuard<'a, T> {}

impl<'a, T: ?Sized + 'a> RwLockUpgradableReadGuard<'a, T> {
    /// Returns a reference to the original reader-writer lock object.
    pub fn rwlock(s: &Self) -> &'a RwLock<T> {
        s.rwlock
    }

    /// Atomically upgrades an upgradable read lock lock into an exclusive write lock,
    /// blocking the current thread until it can be acquired.
    pub fn upgrade(s: Self) -> RwLockWriteGuard<'a, T> {
        // Safety: An RwLockUpgradableReadGuard always holds an upgradable lock.
        unsafe {
            s.rwlock.upgrade();
        }
        let rwlock = s.rwlock;
        mem::forget(s);
        RwLockWriteGuard { rwlock }
    }

    /// Tries to atomically upgrade an upgradable read lock into an exclusive write lock.
    ///
    /// # Errors
    ///
    /// If the access could not be granted at this time, then the current guard is returned.
    pub fn try_upgrade(s: Self) -> Result<RwLockWriteGuard<'a, T>, Self> {
        // Safety: An RwLockUpgradableReadGuard always holds an upgradable lock.
        if unsafe { s.rwlock.try_upgrade() } {
            let rwlock = s.rwlock;
            mem::forget(s);
            Ok(RwLockWriteGuard { rwlock })
        } else {
            Err(s)
        }
    }

    /// Atomically downgrades an upgradable read lock lock into a shared read lock
    /// without allowing any writers to take exclusive access of the lock in the
    /// meantime.
    ///
    /// Note that if there are any writers currently waiting to take the lock
    /// then other readers may not be able to acquire the lock even if it was
    /// downgraded.
    pub fn downgrade(s: Self) -> RwLockReadGuard<'a, T> {
        let data = s.data;
        // Safety: An RwLockUpgradableReadGuard always holds an upgradable lock.
        unsafe {
            s.rwlock.downgrade_upgradable();
        }
        let rwlock = s.rwlock;
        mem::forget(s);
        RwLockReadGuard {
            _data: data,
            rwlock,
        }
    }
}

impl<'a, T: ?Sized + 'a> Deref for RwLockUpgradableReadGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.rwlock.data.with(|data| {
            // Safety: RwLockUpgradableReadGuard always holds a read lock, so obtaining an immutable reference
            // is safe
            unsafe { &*data }
        })
    }
}

impl<'a, T: ?Sized + 'a> Drop for RwLockUpgradableReadGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: An RwLockUpgradableReadGuard always holds an upgradable lock.
        unsafe {
            self.rwlock.unlock_upgradable();
        }
    }
}

impl<'a, T: fmt::Debug + ?Sized + 'a> fmt::Debug for RwLockUpgradableReadGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: fmt::Display + ?Sized + 'a> fmt::Display for RwLockUpgradableReadGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
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
    use super::*;
    use crate::loom::thread;
    use crate::loom::Arc;
    use core::fmt::Debug;
    use core::mem;
    use std::hint;
    use std::sync::mpsc::channel;

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
