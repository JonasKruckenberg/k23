// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;

use util::loom_const_fn;

use crate::Mutex;
use crate::util::hold_interrupts;

/// A mutual exclusion primitive that additionally masks interrupts for the
/// duration of the critical section.
///
/// Use this instead of [`Mutex`] whenever the protected data is also touched
/// from an interrupt handler.
pub struct IrqMutex<T: ?Sized> {
    inner: Mutex<T>,
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
    /// Masks interrupts, acquires the IrqMutex spinning until it is available,
    /// and calls `f` with exclusive access to the protected data.
    ///
    /// The lock is released and the previous interrupt state restored once `f`
    /// returns, including when `f` unwinds.
    #[inline]
    pub fn with_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        // Disable IRQs first, THEN acquire the spinlock.
        // Reversing the order would leave a window where the ISR fires after
        // the spinlock is acquired but before IRQs are masked — same deadlock.
        //
        // Unwinding happens in the same order: `Mutex::with` releases the
        // spinlock, then `_held_irq` drops and restores IRQs. Restoring IRQs
        // while still holding the lock would reopen that window.
        let _held_irq = hold_interrupts();

        self.inner.with_lock(f)
    }

    /// Masks interrupts and attempts to acquire the IrqMutex without spinning;
    /// on success calls `f` with exclusive access to the protected data.
    ///
    /// Returns `None` without running `f` if the IrqMutex is currently locked,
    /// restoring the previous interrupt state immediately.
    #[inline]
    pub fn try_with_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        let _held_irq = hold_interrupts();

        self.inner.try_with_lock(f)
    }

    /// Returns a mutable reference to the underlying data.
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
        // Bind the result before matching: the closure borrows `f` mutably, and
        // a match scrutinee's temporaries would keep that borrow alive across
        // the arms.
        let locked =
            self.try_with_lock(|data| f.debug_struct("IrqMutex").field("data", &&*data).finish());

        match locked {
            Some(res) => res,
            None => f
                .debug_struct("IrqMutex")
                .field("data", &format_args!("<locked>"))
                .finish(),
        }
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
                static ref M: IrqMutex<()> = IrqMutex::new(());
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
                static ref M: IrqMutex<()> = IrqMutex::new(());
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
        let m = IrqMutex::new(());
        m.with_lock(|_| ());
        m.with_lock(|_| ());
    }

    #[test]
    #[cfg_attr(loom, ignore = "not concurrency-relevant")]
    fn try_lock() {
        let m = IrqMutex::new(42);

        assert_eq!(m.try_with_lock(|v| *v), Some(42));

        // A nested `try_with` sees the lock held by the outer critical section.
        let nested = m.try_with_lock(|_| m.try_with_lock(|v| *v));
        assert_eq!(nested, Some(None));

        // ... and the lock is free again afterwards.
        assert_eq!(m.try_with_lock(|v| *v), Some(42));
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
}
