// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// Mutex
// A mutual exclusion primitive useful for protecting shared data
// MutexGuard
// An RAII implementation of a “scoped lock” of a mutex. When this structure is dropped (falls out of scope), the lock will be unlocked.
// MappedMutexGuard
// An RAII mutex guard returned by MutexGuard::map, which can point to a subfield of the protected data.
// ArcMutexGuardarc_lock
// An RAII mutex guard returned by the Arc locking operations on Mutex.

use crate::backoff::Backoff;
use crate::loom::Ordering;
use crate::loom::{AtomicBool, UnsafeCell};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::{fmt, mem};
use util::loom_const_fn;

/// A mutual exclusion primitive useful for protecting shared data
///
/// This mutex will block threads waiting for the lock to become available. The
/// mutex can also be statically initialized or created via a `new`
/// constructor. Each mutex has a type parameter which represents the data that
/// it is protecting. The data can only be accessed through the RAII guards
/// returned from `lock` and `try_lock`, which guarantees that the data is only
/// ever accessed when the mutex is locked.
pub struct Mutex<T: ?Sized> {
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

/// An RAII implementation of a "scoped lock" of a mutex. When this structure is
/// dropped (falls out of scope), the lock will be unlocked.
///
/// The data protected by the mutex can be accessed through this guard via its
/// `Deref` and `DerefMut` implementations.
#[clippy::has_significant_drop]
#[must_use = "if unused the Mutex will immediately unlock"]
pub struct MutexGuard<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
    marker: PhantomData<&'a mut T>,
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    loom_const_fn! {
        pub const fn new(val: T) -> Mutex<T> {
            Mutex {
                lock: AtomicBool::new(false),
                data: UnsafeCell::new(val),
            }
        }
    }

    /// Consumes this mutex, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Creates a new `MutexGuard` without checking if the mutex is locked.
    ///
    /// # Safety
    ///
    /// This method must only be called if the thread logically holds the lock.
    ///
    /// Calling this function when a guard has already been produced is undefined behaviour unless
    /// the guard was forgotten with `mem::forget`.
    #[inline]
    pub unsafe fn make_guard_unchecked(&self) -> MutexGuard<'_, T> {
        MutexGuard {
            mutex: self,
            marker: PhantomData,
        }
    }

    /// Acquires a mutex, blocking the current thread until it is able to do so.
    ///
    /// This function will block the local thread until it is available to acquire
    /// the mutex. Upon returning, the thread is the only thread with the mutex
    /// held. An RAII guard is returned to allow scoped unlock of the lock. When
    /// the guard goes out of scope, the mutex will be unlocked.
    ///
    /// Attempts to lock a mutex in the thread which already holds the lock will
    /// result in a deadlock.
    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
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

        // Safety: The lock is held, as required.
        unsafe { self.make_guard_unchecked() }
    }

    /// Attempts to acquire this lock.
    ///
    /// If the lock could not be acquired at this time, then `None` is returned.
    /// Otherwise, an RAII guard is returned. The lock will be unlocked when the
    /// guard is dropped.
    ///
    /// This function does not block.
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            // SAFETY: The lock is held, as required.
            Some(unsafe { self.make_guard_unchecked() })
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// Since this call borrows the `Mutex` mutably, no actual locking needs to
    /// take place---the mutable borrow statically guarantees no locks exist.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        // Safety: We hold a mutable reference to the Mutex so getting a mutable reference to the
        // data is safe
        self.data.with_mut(|data| unsafe { &mut *data })
    }

    /// Checks whether the mutex is currently locked.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    /// Forcibly unlocks the mutex.
    ///
    /// This is useful when combined with `mem::forget` to hold a lock without
    /// the need to maintain a `MutexGuard` object alive, for example when
    /// dealing with FFI.
    ///
    /// # Safety
    ///
    /// This method must only be called if the current thread logically owns a
    /// `MutexGuard` but that guard has been discarded using `mem::forget`.
    /// Behavior is undefined if a mutex is unlocked when not locked.
    #[inline]
    pub unsafe fn force_unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

impl<T: Default> Default for Mutex<T> {
    #[inline]
    fn default() -> Mutex<T> {
        Mutex::new(Default::default())
    }
}

impl<T> From<T> for Mutex<T> {
    #[inline]
    fn from(t: T) -> Mutex<T> {
        Mutex::new(t)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => f.debug_struct("Mutex").field("data", &&*guard).finish(),
            None => {
                struct LockedPlaceholder;
                impl fmt::Debug for LockedPlaceholder {
                    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("<locked>")
                    }
                }

                f.debug_struct("Mutex")
                    .field("data", &LockedPlaceholder)
                    .finish()
            }
        }
    }
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<'a, T: ?Sized + Sync + 'a> Sync for MutexGuard<'a, T> {}

impl<'a, T: ?Sized + 'a> MutexGuard<'a, T> {
    /// Returns a reference to the original `Mutex` object.
    pub fn mutex(s: &Self) -> &'a Mutex<T> {
        s.mutex
    }

    /// Leaks the mutex guard and returns a mutable reference to the data
    /// protected by the mutex.
    ///
    /// This will leave the `Mutex` in a locked state.
    #[inline]
    pub fn leak(s: Self) -> &'a mut T {
        // Safety: MutexGuard always holds the lock, so it is safe to access the data
        let r = s.mutex.data.with_mut(|r| unsafe { &mut *r });
        mem::forget(s);
        r
    }

    pub fn unlocked<F, U>(s: &mut Self, f: F) -> U
    where
        F: FnOnce() -> U,
    {
        struct DropGuard<'a, T: ?Sized> {
            mutex: &'a Mutex<T>,
        }
        impl<T: ?Sized> Drop for DropGuard<'_, T> {
            fn drop(&mut self) {
                mem::forget(self.mutex.lock());
            }
        }

        // Safety: A MutexGuard always holds the lock.
        unsafe {
            s.mutex.force_unlock();
        }
        let _guard = DropGuard { mutex: s.mutex };
        f()
    }
}

impl<'a, T: ?Sized + 'a> Deref for MutexGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // Safety: MutexGuard always holds the lock, so it is safe to access the data
        self.mutex.data.with(|data| unsafe { &*data })
    }
}

impl<'a, T: ?Sized + 'a> DerefMut for MutexGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // Safety: MutexGuard always holds the lock, so it is safe to access the data
        self.mutex.data.with_mut(|data| unsafe { &mut *data })
    }
}

impl<'a, T: ?Sized + 'a> Drop for MutexGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: A MutexGuard always holds the lock.
        unsafe {
            self.mutex.force_unlock();
        }
    }
}

impl<'a, T: fmt::Debug + ?Sized + 'a> fmt::Debug for MutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: fmt::Display + ?Sized + 'a> fmt::Display for MutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

#[cfg(feature = "lock_api")]
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl lock_api::RawMutex for Mutex<()> {
    #[allow(clippy::declare_interior_mutable_const, reason = "TODO")]
    const INIT: Self = Mutex::new(());
    type GuardMarker = lock_api::GuardSend;

    fn lock(&self) {
        let g = Mutex::lock(self);
        mem::forget(g);
    }

    fn try_lock(&self) -> bool {
        let g = Mutex::try_lock(self);
        let ret = g.is_some();
        mem::forget(g);
        ret
    }

    unsafe fn unlock(&self) {
        // Safety: ensured by caller
        unsafe {
            Mutex::force_unlock(self);
        }
    }

    fn is_locked(&self) -> bool {
        Mutex::is_locked(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom::Arc;
    use crate::loom::AtomicUsize;
    use core::fmt::Debug;
    use std::{hint, mem};

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
        let m = Mutex::new(());
        drop(m.lock());
        drop(m.lock());
    }

    #[test]
    fn try_lock() {
        let mutex = Mutex::<_>::new(42);

        // First lock succeeds
        let a = mutex.try_lock();
        assert_eq!(a.as_ref().map(|r| **r), Some(42));

        // Additional lock fails
        let b = mutex.try_lock();
        assert!(b.is_none());

        // After dropping lock, it succeeds again
        drop(a);
        let c = mutex.try_lock();
        assert_eq!(c.as_ref().map(|r| **r), Some(42));
    }

    #[test]
    fn test_into_inner() {
        let m = Mutex::<_>::new(NonCopy(10));
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
        let m = Mutex::<_>::new(Foo(num_drops.clone()));
        assert_eq!(num_drops.load(Ordering::SeqCst), 0);
        {
            let _inner = m.into_inner();
            assert_eq!(num_drops.load(Ordering::SeqCst), 0);
        }
        assert_eq!(num_drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_mutex_unsized() {
        let mutex: &Mutex<[i32]> = &Mutex::<_>::new([1, 2, 3]);
        {
            let b = &mut *mutex.lock();
            b[0] = 4;
            b[2] = 5;
        }
        let comp: &[i32] = &[4, 2, 5];
        assert_eq!(&*mutex.lock(), comp);
    }

    #[test]
    fn test_mutex_force_lock() {
        let lock = Mutex::<_>::new(());
        mem::forget(lock.lock());
        unsafe {
            lock.force_unlock();
        }
        assert!(lock.try_lock().is_some());
    }

    #[test]
    fn test_get_mut() {
        let mut m = Mutex::new(NonCopy(10));
        *m.get_mut() = NonCopy(20);
        assert_eq!(m.into_inner(), NonCopy(20));
    }

    #[test]
    fn basic_multi_threaded() {
        use crate::loom::{self, Arc, thread};

        #[allow(tail_expr_drop_order)]
        fn incr(lock: &Arc<Mutex<i32>>) -> thread::JoinHandle<()> {
            let lock = lock.clone();
            thread::spawn(move || {
                let mut lock = lock.lock();
                *lock += 1;
            })
        }

        loom::model(|| {
            let lock = Arc::new(Mutex::new(0));
            let t1 = incr(&lock);
            let t2 = incr(&lock);

            t1.join().unwrap();
            t2.join().unwrap();

            thread::spawn(move || {
                let lock = lock.lock();
                assert_eq!(*lock, 2)
            });
        });
    }
}
