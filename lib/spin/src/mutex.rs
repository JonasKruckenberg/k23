// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::{fmt, hint};

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
/// # Examples
///
/// ```
/// let mutex = spin::Mutex::new(0);
///
/// mutex.with_lock(|val| *val += 1);
/// assert_eq!(mutex.with_lock(|val| *val), 1);
/// ```
pub struct Mutex<T: ?Sized> {
    lock: AtomicBool,
    data: UnsafeCell<T>,
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
    /// Acquires the mutex, spinning until it is available, and calls `f` with
    /// exclusive access to the protected data.
    ///
    /// The lock is released once `f` returns, including when `f` unwinds.
    #[inline]
    pub fn with_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        self.raw_lock();
        let _unlock = Unlock(self);

        // Safety: the lock is held, so no other thread can alias the data.
        self.data.with_mut(|data| f(unsafe { &mut *data }))
    }

    /// Attempts to acquire the mutex without spinning and, on success, calls
    /// `f` with exclusive access to the protected data.
    ///
    /// Returns `None` without running `f` if the mutex is currently locked.
    /// Otherwise the lock is released once `f` returns, including when `f`
    /// unwinds.
    #[inline]
    pub fn try_with_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        if !self.raw_try_lock(true) {
            return None;
        }
        let _unlock = Unlock(self);

        // Safety: the lock is held, so no other thread can alias the data.
        Some(self.data.with_mut(|data| f(unsafe { &mut *data })))
    }

    /// Returns a mutable reference to the underlying data.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        // Safety: exclusive borrow of self means no critical section can be running.
        self.data.with_mut(|data| unsafe { &mut *data })
    }

    /// Returns `true` if the mutex is currently locked.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    /// Acquires the lock, spinning until it is available.
    #[inline]
    fn raw_lock(&self) {
        let mut boff = Backoff::new();
        while !self.raw_try_lock(false) {
            hint::cold_path();
            while self.is_locked() {
                boff.spin();
            }
        }
    }

    #[inline(always)]
    fn raw_try_lock(&self, strong: bool) -> bool {
        // Loom models a weak exchange as able to fail spuriously on *every*
        // attempt, so the retry loop in `raw_lock` has no bound and the model
        // never terminates. Go strong under loom: a spurious failure exposes no
        // interleaving that a strong compare-exchange does not.
        if strong || cfg!(loom) {
            self.lock
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        } else {
            self.lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        }
    }

    /// Releases the lock.
    ///
    /// # Safety
    ///
    /// The caller must hold the lock and must not release it by other means.
    #[inline]
    unsafe fn raw_unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

/// Releases the mutex when dropped, so an unwind out of a `with` closure
/// cannot leave the lock held forever.
struct Unlock<'a, T: ?Sized>(&'a Mutex<T>);

impl<T: ?Sized> Drop for Unlock<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: `Unlock` is only constructed after this thread acquired the
        // lock, and nothing else releases it in between.
        unsafe { self.0.raw_unlock() }
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
        // Bind the result before matching: the closure borrows `f` mutably, and
        // a match scrutinee's temporaries would keep that borrow alive across
        // the arms.
        let locked =
            self.try_with_lock(|data| f.debug_struct("Mutex").field("data", &&*data).finish());

        match locked {
            Some(res) => res,
            None => f
                .debug_struct("Mutex")
                .field("data", &format_args!("<locked>"))
                .finish(),
        }
    }
}

// `lock_api::RawMutex::INIT` is an associated const, which forces
// `Mutex::new(())` to be a constant expression. That rules out loom atomics
// (their constructors aren't `const`), so this impl is only compiled outside
// loom. The kernel (`talc::TalcLock<spin::RawMutex, _>`) never builds under
// loom, so it's unaffected; loom-driven tests in this crate use `Mutex`
// directly and don't need the trait.
#[cfg(not(loom))]
// Safety: standard spinlock semantics; `unlock` is only reachable via the
// trait from a caller that logically holds the lock.
unsafe impl lock_api::RawMutex for Mutex<()> {
    type GuardMarker = lock_api::GuardSend;

    #[allow(clippy::declare_interior_mutable_const, reason = "required by trait")]
    const INIT: Self = Mutex::new(());

    fn lock(&self) {
        self.raw_lock();
    }

    fn try_lock(&self) -> bool {
        self.raw_try_lock(true)
    }

    unsafe fn unlock(&self) {
        // Safety: caller contract of `lock_api::RawMutex::unlock`.
        unsafe { self.raw_unlock() };
    }

    fn is_locked(&self) -> bool {
        Mutex::is_locked(self)
    }
}

#[cfg(test)]
mod tests {
    use fastrand::FastRand;

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
        10
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
            for i in 0..THREADS {
                threads.push(thread::spawn(move || {
                    let mut rng = FastRand::from_seed(i as u64 + 1);
                    for _ in 0..CYCLES {
                        M.with_lock(|buf| {
                            assert!(buf.iter().all(|b| *b == buf[0]));

                            buf.fill(rng.fastrand().to_le_bytes()[0]);
                        });

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
                        M.with_lock(|_| {
                            assert_eq!(DATA.fetch_add(1, Ordering::Relaxed), 0);
                            assert_eq!(DATA.fetch_sub(1, Ordering::Relaxed), 1);
                        });

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
                        loop {
                            let acquired = M.try_with_lock(|_| {
                                assert_eq!(DATA.fetch_add(1, Ordering::Relaxed), 0);
                                assert_eq!(DATA.fetch_sub(1, Ordering::Relaxed), 1);
                            });
                            if acquired.is_some() {
                                break;
                            }
                            boff.spin();
                        }

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
        m.with_lock(|_| ());
        m.with_lock(|_| ());
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn try_lock() {
        let m = Mutex::new(42);

        assert_eq!(m.try_with_lock(|v| *v), Some(42));

        // A nested `try_with` sees the lock held by the outer critical section.
        let nested = m.try_with_lock(|_| m.try_with_lock(|v| *v));
        assert_eq!(nested, Some(None));

        // ... and the lock is free again afterwards.
        assert_eq!(m.try_with_lock(|v| *v), Some(42));
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn released_on_unwind() {
        let m = Mutex::new(42);

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            m.with_lock(|v| {
                *v = 7;
                panic!("boom");
            })
        }));

        assert!(res.is_err());
        assert!(!m.is_locked(), "the lock must be released on unwind");
        assert_eq!(m.try_with_lock(|v| *v), Some(7));
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
}
