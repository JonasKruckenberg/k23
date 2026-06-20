// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::ops::{Deref, DerefMut};

use util::loom_const_fn;

use crate::util::{HeldInterrupts, hold_interrupts};
use crate::{Mutex, MutexGuard};

/// A mutual exclusion primitive useful for protecting shared data.
///
/// This IrqMutex will spin waiting for the lock to become available. Each IrqMutex
/// has a type parameter which represents the data it is protecting. The data
/// can only be accessed through the RAII guards returned from `lock` and
/// `try_lock`.
pub struct IrqMutex<T: ?Sized> {
    inner: Mutex<T>,
}

/// An RAII implementation of a "scoped lock" of a [`IrqMutex`]. When this
/// structure is dropped (falls out of scope), the lock will be unlocked.
///
/// The data protected by the IrqMutex can be accessed through this guard via its
/// `Deref` and `DerefMut` implementations.
#[clippy::has_significant_drop]
#[must_use = "if unused the IrqMutex will immediately unlock"]
pub struct IrqMutexGuard<'a, T: ?Sized> {
    guard: MutexGuard<'a, T>,
    // Declared after `guard` so it is dropped *after* it: the spinlock is
    // released first, then IRQs are restored (Rust drops fields in declaration
    // order). Reversing this would restore IRQs while still holding the lock.
    _held_irq: HeldInterrupts,
}

// Safety: IrqMutex provides mutual exclusion over T.
unsafe impl<T: ?Sized + Send> Send for IrqMutex<T> {}
// Safety: IrqMutex provides mutual exclusion over T.
unsafe impl<T: ?Sized + Send> Sync for IrqMutex<T> {}

impl<T> IrqMutex<T> {
    loom_const_fn! {
        /// Creates a new IrqMutex in an unlocked state.
        pub const fn new(val: T) -> IrqMutex<T> {
            IrqMutex {
                inner: Mutex::new(val)
            }
        }
    }

    /// Consumes this IrqMutex, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

impl<T: ?Sized> IrqMutex<T> {
    /// Acquires the IrqMutex, spinning until it is available.
    #[inline]
    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        // Disable IRQs first, THEN acquire the spinlock.
        // Reversing the order would leave a window where the ISR fires after
        // the spinlock is acquired but before IRQs are masked — same deadlock.
        let _held_irq = hold_interrupts();

        IrqMutexGuard {
            guard: self.inner.lock(),
            _held_irq,
        }
    }

    /// Attempts to acquire the IrqMutex without spinning.
    ///
    /// Returns `None` if the IrqMutex is currently locked.
    #[inline]
    pub fn try_lock(&self) -> Option<IrqMutexGuard<'_, T>> {
        let _held_irq = hold_interrupts();

        // If the lock is taken, `_held_irq` is dropped here, restoring IRQs;
        // otherwise it moves into the guard and restores them on unlock.
        self.inner
            .try_lock()
            .map(|guard| IrqMutexGuard { guard, _held_irq })
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// Since this call borrows the IrqMutex mutably, no locking is needed.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    /// Returns `true` if the IrqMutex is currently locked.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }
}

impl<T: Default> Default for IrqMutex<T> {
    #[inline]
    fn default() -> IrqMutex<T> {
        IrqMutex::new(T::default())
    }
}

impl<T> From<T> for IrqMutex<T> {
    #[inline]
    fn from(t: T) -> IrqMutex<T> {
        IrqMutex::new(t)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for IrqMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => f.debug_struct("IrqMutex").field("data", &&*guard).finish(),
            None => f
                .debug_struct("IrqMutex")
                .field("data", &format_args!("<locked>"))
                .finish(),
        }
    }
}

impl<'a, T: ?Sized + 'a> IrqMutexGuard<'a, T> {
    /// Temporarily unlocks the IrqMutex to execute the given closure.
    ///
    /// The IrqMutex is re-acquired before this method returns.
    pub fn unlocked<F, U>(s: &mut Self, f: F) -> U
    where
        F: FnOnce() -> U,
    {
        // Fully release for the duration of `f`: the inner guard unlocks (and
        // re-locks) the spinlock, and we restore the prior IRQ state around it so
        // the closure runs with the lock free and interrupts as they were before.
        MutexGuard::unlocked(&mut s.guard, || s._held_irq.with_released(f))
    }
}

impl<'a, T: ?Sized + 'a> Deref for IrqMutexGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.guard.deref()
    }
}

impl<'a, T: ?Sized + 'a> DerefMut for IrqMutexGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.guard.deref_mut()
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for IrqMutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for IrqMutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use fastrand::FastRand;

    use super::*;
    use crate::loom::sync::atomic::{AtomicUsize, Ordering};
    use crate::loom::thread;
    use crate::{Backoff, loom};

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
        /// Size of the IrqMutex-protected data; miri is slow for large buffers.
        const BUF_SIZE: usize = if cfg!(miri) { 8 } else { 1024 };

        loom::lazy_static! {
            static ref M: IrqMutex<[u8; BUF_SIZE]> = IrqMutex::new([0u8; BUF_SIZE]);
        }

        loom::model(|| {
            let mut threads = Vec::new();
            for i in 0..THREADS {
                threads.push(thread::spawn(move || {
                    let mut rng = FastRand::from_seed(i as u64 + 1);
                    for _ in 0..CYCLES {
                        let mut guard = M.lock();

                        assert!(guard.iter().all(|b| *b == guard[0]));

                        guard.fill(rng.fastrand().to_le_bytes()[0]);

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
                static ref M: IrqMutex<()> = IrqMutex::new(());
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
                static ref M: IrqMutex<()> = IrqMutex::new(());
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
        let m = IrqMutex::new(());
        drop(m.lock());
        drop(m.lock());
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn try_lock() {
        let m = IrqMutex::new(42);

        let a = m.try_lock();
        assert_eq!(a.as_ref().map(|r| **r), Some(42));

        let b = m.try_lock();
        assert!(b.is_none());

        drop(a);
        let c = m.try_lock();
        assert_eq!(c.as_ref().map(|r| **r), Some(42));
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn into_inner() {
        let m = IrqMutex::new(42);
        assert_eq!(m.into_inner(), 42);
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn get_mut() {
        let mut m = IrqMutex::new(10);
        *m.get_mut() = 20;
        assert_eq!(m.into_inner(), 20);
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn unlocked_smoke() {
        let m = IrqMutex::new(0);
        let mut g = m.lock();
        *g = 1;

        let side_effect = IrqMutexGuard::unlocked(&mut g, || {
            // Because the guard released the lock, another try_lock would succeed.
            assert!(m.try_lock().is_some());
            42
        });

        assert_eq!(side_effect, 42);
        assert_eq!(*g, 1);
    }
}
