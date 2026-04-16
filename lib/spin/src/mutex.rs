// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::{fmt, hint, mem};

use util::loom_const_fn;

use crate::Backoff;
use crate::loom::cell::UnsafeCell;
use crate::loom::sync::atomic::{AtomicBool, Ordering};

/// Type alias for a unit-valued [`Mutex`], exposed to give downstream crates
/// (notably the `talc` kernel allocator) a concrete [`lock_api::RawMutex`]
/// implementation.
pub type RawMutex = Mutex<()>;

/// A mutual exclusion primitive useful for protecting shared data.
///
/// This mutex will spin waiting for the lock to become available. Each mutex
/// has a type parameter which represents the data it is protecting. The data
/// can only be accessed through the RAII guards returned from `lock` and
/// `try_lock`.
pub struct Mutex<T: ?Sized> {
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

/// An RAII implementation of a "scoped lock" of a [`Mutex`]. When this
/// structure is dropped (falls out of scope), the lock will be unlocked.
///
/// The data protected by the mutex can be accessed through this guard via its
/// `Deref` and `DerefMut` implementations.
#[clippy::has_significant_drop]
#[must_use = "if unused the Mutex will immediately unlock"]
pub struct MutexGuard<'a, T: ?Sized> {
    mutex: &'a Mutex<T>,
    marker: PhantomData<&'a mut T>,
}

// Safety: Mutex provides mutual exclusion over T.
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
// Safety: Mutex provides mutual exclusion over T.
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    loom_const_fn! {
        /// Creates a new mutex in an unlocked state.
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
    /// Acquires the mutex, spinning until it is available.
    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        let mut boff = Backoff::new();
        while !self.try_lock_internal(false) {
            hint::cold_path();
            while self.is_locked() {
                boff.spin();
            }
        }

        // Safety: the lock is held.
        unsafe { self.make_guard_unchecked() }
    }

    /// Attempts to acquire the mutex without spinning.
    ///
    /// Returns `None` if the mutex is currently locked.
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self.try_lock_internal(true) {
            // Safety: the lock is held.
            Some(unsafe { self.make_guard_unchecked() })
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// Since this call borrows the mutex mutably, no locking is needed.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        // Safety: exclusive borrow of self means no guard can be outstanding.
        self.data.with_mut(|data| unsafe { &mut *data })
    }

    /// Returns `true` if the mutex is currently locked.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    /// Creates a new `MutexGuard` without checking that the lock is held.
    ///
    /// # Safety
    ///
    /// The caller must logically hold the lock; creating two guards for the
    /// same lock at the same time is UB unless the first has been forgotten.
    #[inline]
    unsafe fn make_guard_unchecked(&self) -> MutexGuard<'_, T> {
        MutexGuard {
            mutex: self,
            marker: PhantomData,
        }
    }

    /// Forcibly unlocks the mutex.
    ///
    /// # Safety
    ///
    /// Must only be called when the current thread logically owns a
    /// `MutexGuard` but has discarded it via `mem::forget`.
    #[inline]
    unsafe fn force_unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    #[inline(always)]
    fn try_lock_internal(&self, strong: bool) -> bool {
        if strong {
            self.lock
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        } else {
            self.lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        }
    }
}

impl<T: Default> Default for Mutex<T> {
    #[inline]
    fn default() -> Mutex<T> {
        Mutex::new(T::default())
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
            None => f
                .debug_struct("Mutex")
                .field("data", &format_args!("<locked>"))
                .finish(),
        }
    }
}

// Safety: access to T is serialized by the Mutex.
unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<'a, T: ?Sized + 'a> MutexGuard<'a, T> {
    /// Temporarily unlocks the mutex to execute the given closure.
    ///
    /// The mutex is re-acquired before this method returns.
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

        // Safety: the guard owns the lock.
        unsafe {
            s.mutex.force_unlock();
        }
        let _drop_guard = DropGuard { mutex: s.mutex };
        f()
    }
}

impl<'a, T: ?Sized + 'a> Deref for MutexGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // Safety: the guard holds the lock.
        self.mutex.data.with(|data| unsafe { &*data })
    }
}

impl<'a, T: ?Sized + 'a> DerefMut for MutexGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // Safety: the guard holds the lock exclusively.
        self.mutex.data.with_mut(|data| unsafe { &mut *data })
    }
}

impl<'a, T: ?Sized + 'a> Drop for MutexGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: the guard holds the lock.
        unsafe {
            self.mutex.force_unlock();
        }
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

// `lock_api::RawMutex::INIT` is an associated const, which forces
// `Mutex::new(())` to be a constant expression. That rules out loom atomics
// (their constructors aren't `const`), so this impl is only compiled outside
// loom. The kernel (`talc::Talck<spin::RawMutex, _>`) never builds under loom,
// so it's unaffected; loom-driven tests in this crate use `Mutex` directly and
// don't need the trait.
#[cfg(not(loom))]
// Safety: standard spinlock semantics; `unlock` is only reachable via the
// trait from a caller that logically holds the lock.
unsafe impl lock_api::RawMutex for Mutex<()> {
    type GuardMarker = lock_api::GuardSend;

    #[allow(clippy::declare_interior_mutable_const, reason = "required by trait")]
    const INIT: Self = Mutex::new(());

    fn lock(&self) {
        mem::forget(Mutex::lock(self));
    }

    fn try_lock(&self) -> bool {
        match Mutex::try_lock(self) {
            Some(g) => {
                mem::forget(g);
                true
            }
            None => false,
        }
    }

    unsafe fn unlock(&self) {
        // Safety: caller contract of `lock_api::RawMutex::unlock`.
        unsafe { self.force_unlock() };
    }

    fn is_locked(&self) -> bool {
        Mutex::is_locked(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom;
    use crate::loom::sync::atomic::AtomicUsize;
    use crate::loom::thread;

    /// Number of cycles to repeat concurrency tests for. Loom's state space
    /// blows up combinatorially — one cycle is enough to cover every
    /// interesting interleaving, more just wastes hours.
    const CYCLES: usize = if cfg!(loom) {
        1
    } else if cfg!(miri) {
        100
    } else {
        500
    };

    /// Threads to spawn in concurrency tests. Loom's model checker explores
    /// every possible interleaving, so the state space scales exponentially
    /// with thread count. Two threads are enough to exercise every race a
    /// spinlock can produce.
    const THREADS: usize = if cfg!(loom) { 2 } else { loom::MAX_THREADS - 1 };

    #[test]
    fn correctness() {
        /// Size of the mutex-protected data; miri is slow for large buffers.
        const BUF_SIZE: usize = if cfg!(miri) { 8 } else { 1024 };

        loom::lazy_static! {
            static ref M: Mutex<[u8; BUF_SIZE]> = Mutex::new([0u8; BUF_SIZE]);
        }

        loom::model(|| {
            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let mut guard = M.lock();

                        assert!(guard.iter().all(|b| *b == guard[0]));

                        guard.fill(rand::random());

                        drop(guard);
                        #[cfg(loom)]
                        thread::yield_now();
                    }
                }))
            }

            for t in threads {
                t.join().unwrap();
            }
        })
    }

    #[test]
    fn stress_test() {
        loom::model(|| {
            loom::lazy_static! {
                static ref M: Mutex<()> = Mutex::new(());
                static ref DATA: AtomicUsize = AtomicUsize::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let guard = M.lock();
                        assert_eq!(DATA.fetch_add(1, Ordering::Relaxed), 0);
                        assert_eq!(DATA.fetch_sub(1, Ordering::Relaxed), 1);
                        drop(guard);

                        #[cfg(loom)]
                        thread::yield_now();
                    }
                }));
            }

            for t in threads {
                t.join().unwrap();
            }
        })
    }

    #[test]
    fn stress_test_try_lock() {
        loom::model(|| {
            loom::lazy_static! {
                static ref M: Mutex<()> = Mutex::new(());
                static ref DATA: AtomicUsize = AtomicUsize::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let mut boff = Backoff::new();
                        let guard = loop {
                            if let Some(g) = M.try_lock() {
                                break g;
                            }
                            boff.spin();
                        };

                        assert_eq!(DATA.fetch_add(1, Ordering::Relaxed), 0);
                        assert_eq!(DATA.fetch_sub(1, Ordering::Relaxed), 1);
                        drop(guard);

                        #[cfg(loom)]
                        thread::yield_now();
                    }
                }));
            }

            for t in threads {
                t.join().unwrap();
            }
        })
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn smoke() {
        let m = Mutex::new(());
        drop(m.lock());
        drop(m.lock());
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn try_lock() {
        let mutex = Mutex::new(42);

        let a = mutex.try_lock();
        assert_eq!(a.as_ref().map(|r| **r), Some(42));

        let b = mutex.try_lock();
        assert!(b.is_none());

        drop(a);
        let c = mutex.try_lock();
        assert_eq!(c.as_ref().map(|r| **r), Some(42));
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn into_inner() {
        let m = Mutex::new(42);
        assert_eq!(m.into_inner(), 42);
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn get_mut() {
        let mut m = Mutex::new(10);
        *m.get_mut() = 20;
        assert_eq!(m.into_inner(), 20);
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn unlocked_smoke() {
        let m = Mutex::new(0);
        let mut g = m.lock();
        *g = 1;

        let side_effect = MutexGuard::unlocked(&mut g, || {
            // Because the guard released the lock, another try_lock would succeed.
            assert!(m.try_lock().is_some());
            42
        });

        assert_eq!(side_effect, 42);
        assert_eq!(*g, 1);
    }
}
