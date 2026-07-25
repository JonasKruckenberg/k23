// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! A lock that provides data access to either one writer or many readers, and
//! masks interrupts for the duration of the critical section.

use core::fmt;

use util::loom_const_fn;

use crate::util::hold_interrupts;
use crate::{RwLock, Upgradeable};

/// A lock that provides data access to either one writer or many readers, and
/// additionally masks interrupts for the duration of the critical section.
///
/// Use this instead of [`RwLock`] whenever the protected data is also touched
/// from an interrupt handler.
///
/// # Examples
///
/// ```
/// use spin;
///
/// let lock = spin::IrqRwLock::new(5);
///
/// // many reader locks can be held at once
/// lock.with_read_lock(|r1| {
///     lock.with_read_lock(|r2| {
///         assert_eq!(*r1, 5);
///         assert_eq!(*r2, 5);
///     });
/// }); // read locks are released here
///
/// // only one write lock may be held, however
/// lock.with_write_lock(|w| {
///     *w += 1;
///     assert_eq!(*w, 6);
/// }); // write lock is released here
/// ```
pub struct IrqRwLock<T: ?Sized> {
    inner: RwLock<T>,
}

impl<T> IrqRwLock<T> {
    loom_const_fn! {
        /// Creates a new spinlock wrapping the supplied data.
        #[inline]
        pub const fn new(data: T) -> Self {
            IrqRwLock {
                inner: RwLock::new(data)
            }
        }
    }

    /// Consumes this `IrqRwLock`, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        // We know statically that there are no outstanding references to
        // `self` so there's no need to lock.
        let IrqRwLock { inner, .. } = self;
        inner.into_inner()
    }
}

impl<T: ?Sized> IrqRwLock<T> {
    /// Masks interrupts, locks this rwlock with shared read access blocking the
    /// current thread until it can be acquired, and calls `f` with shared
    /// access to the protected data.
    ///
    /// The lock is released and the previous interrupt state restored once `f`
    /// returns, including when `f` unwinds.
    #[inline]
    pub fn with_read_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        // Disable IRQs first, THEN acquire the spinlock.
        // Reversing the order would leave a window where the ISR fires after
        // the spinlock is acquired but before IRQs are masked => deadlock.
        //
        // Unwinding happens in the same order: `RwLock::with_read` releases the
        // lock, then `_held_irq` drops and restores IRQs.
        let _held_irq = hold_interrupts();

        self.inner.with_read_lock(f)
    }

    /// Masks interrupts and attempts to acquire this lock with shared read
    /// access; on success calls `f` with shared access to the protected data.
    ///
    /// Returns `None` without running `f` if [`IrqRwLock::with_read`] would
    /// otherwise block, restoring the previous interrupt state immediately.
    #[inline]
    pub fn try_with_read_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&T) -> R,
    {
        let _held_irq = hold_interrupts();

        self.inner.try_with_read_lock(f)
    }

    /// Masks interrupts, locks this rwlock with exclusive write access blocking
    /// the current thread until it can be acquired, and calls `f` with
    /// exclusive access to the protected data.
    ///
    /// The lock is released and the previous interrupt state restored once `f`
    /// returns, including when `f` unwinds.
    #[inline]
    pub fn with_write_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let _held_irq = hold_interrupts();

        self.inner.with_write_lock(f)
    }

    /// Masks interrupts and attempts to lock this rwlock with exclusive write
    /// access; on success calls `f` with exclusive access to the protected data.
    ///
    /// Returns `None` without running `f` if [`IrqRwLock::with_write`] would
    /// otherwise block, restoring the previous interrupt state immediately.
    #[inline]
    pub fn try_with_write_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        let _held_irq = hold_interrupts();

        self.inner.try_with_write_lock(f)
    }

    /// Masks interrupts and attempts to lock this rwlock with exclusive write
    /// access; on success calls `f` with exclusive access to the protected data.
    ///
    /// Unlike [`IrqRwLock::try_with_write`], this function is allowed to
    /// spuriously fail even when acquiring exclusive write access would
    /// otherwise succeed, which can result in more efficient code on some
    /// platforms.
    #[inline]
    pub fn try_with_write_weak<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        let _held_irq = hold_interrupts();

        self.inner.try_with_write_lock_weak(f)
    }

    /// Masks interrupts, obtains an upgradeable handle blocking the current
    /// thread until it can be acquired, and calls `f` with it.
    ///
    /// Interrupts stay masked for the whole of `f`, including while the handle
    /// is temporarily upgraded via [`Upgradeable::upgrade`] or handed off via
    /// [`Upgradeable::downgrade`].
    #[inline]
    pub fn with_upgradeable_read_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(Upgradeable<'_, T>) -> R,
    {
        let _held_irq = hold_interrupts();

        self.inner.with_upgradeable_read_lock(f)
    }

    /// Masks interrupts and attempts to obtain an upgradeable handle; on
    /// success calls `f` with it.
    ///
    /// Returns `None` without running `f` if a writer or another upgradeable
    /// handle currently holds the lock, restoring the previous interrupt state
    /// immediately.
    #[inline]
    pub fn try_with_upgradeable_read_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(Upgradeable<'_, T>) -> R,
    {
        let _held_irq = hold_interrupts();

        self.inner.try_with_upgradeable_read_lock(f)
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut lock = spin::IrqRwLock::new(0);
    /// *lock.get_mut() = 10;
    /// assert_eq!(lock.with_read_lock(|v| *v), 10);
    /// ```
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    /// Returns `true` if the lock is currently held in any mode.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }

    /// Returns `true` if the lock is held in exclusive mode.
    #[inline]
    pub fn is_locked_exclusive(&self) -> bool {
        self.inner.is_locked_exclusive()
    }

    /// Return the number of readers that currently hold the lock (including upgradable readers).
    ///
    /// # Safety
    ///
    /// This function provides no synchronization guarantees and so its result should be considered 'out of date'
    /// the instant it is called. Do not use it for synchronization purposes. However, it may be useful as a heuristic.
    pub fn reader_count(&self) -> usize {
        self.inner.reader_count()
    }

    /// Return the number of writers that currently hold the lock.
    ///
    /// Because [`IrqRwLock`] guarantees exclusive mutable access, this function may only return either `0` or `1`.
    ///
    /// # Safety
    ///
    /// This function provides no synchronization guarantees and so its result should be considered 'out of date'
    /// the instant it is called. Do not use it for synchronization purposes. However, it may be useful as a heuristic.
    pub fn writer_count(&self) -> usize {
        self.inner.writer_count()
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for IrqRwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Bind the result before matching: the closure borrows `f` mutably, and
        // a match scrutinee's temporaries would keep that borrow alive across
        // the arms.
        let locked = self.try_with_read_lock(|data| {
            write!(f, "IrqRwLock {{ data: ")
                .and_then(|()| data.fmt(f))
                .and_then(|()| write!(f, " }}"))
        });

        match locked {
            Some(res) => res,
            None => write!(f, "IrqRwLock {{ <locked> }}"),
        }
    }
}

impl<T: Default> Default for IrqRwLock<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T> From<T> for IrqRwLock<T> {
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loom;
    use crate::loom::sync::Arc;
    use crate::loom::sync::atomic::{AtomicUsize, Ordering};
    use crate::loom::thread;

    /// Threads to spawn in concurrency tests. Loom's model checker explores
    /// every possible interleaving, so the state space scales exponentially
    /// with thread count. Two threads are enough to exercise every race a
    /// rwlock can produce.
    const THREADS: usize = if cfg!(loom) { 2 } else { loom::MAX_THREADS - 1 };

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

    #[derive(Eq, PartialEq, Debug)]
    struct NonCopy(i32);

    #[test]
    fn smoke() {
        loom::model(|| {
            let l = IrqRwLock::new(());
            l.with_read_lock(|_| ());
            l.with_write_lock(|_| ());
            l.with_read_lock(|_| l.with_read_lock(|_| ()));
            l.with_write_lock(|_| ());
        });
    }

    #[test]
    fn test_rwlock_unsized() {
        loom::model(|| {
            let rw: &IrqRwLock<[i32]> = &IrqRwLock::new([1, 2, 3]);
            rw.with_write_lock(|b| {
                b[0] = 4;
                b[2] = 5;
            });
            let comp: &[i32] = &[4, 2, 5];
            rw.with_read_lock(|b| assert_eq!(b, comp));
        })
    }

    #[test]
    fn test_rwlock_try_write() {
        loom::model(|| {
            let lock = IrqRwLock::new(0isize);
            lock.with_read_lock(|_| {
                assert!(
                    lock.try_with_write_lock(|_| ()).is_none(),
                    "try_with_write should not succeed while a reader is in scope"
                );
            });
        })
    }

    #[test]
    fn test_rw_try_read() {
        loom::model(|| {
            let m = IrqRwLock::new(0);
            m.with_write_lock(|_| {
                assert!(m.try_with_read_lock(|_| ()).is_none());
            });
        })
    }

    #[test]
    fn test_into_inner() {
        loom::model(|| {
            let m = IrqRwLock::new(NonCopy(10));
            assert_eq!(m.into_inner(), NonCopy(10));
        })
    }

    #[test]
    fn test_into_inner_drop() {
        loom::model(|| {
            struct Foo(Arc<AtomicUsize>);
            impl Drop for Foo {
                fn drop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
            let num_drops = Arc::new(AtomicUsize::new(0));
            let m = IrqRwLock::new(Foo(num_drops.clone()));
            assert_eq!(num_drops.load(Ordering::SeqCst), 0);
            {
                let _inner = m.into_inner();
                assert_eq!(num_drops.load(Ordering::SeqCst), 0);
            }
            assert_eq!(num_drops.load(Ordering::SeqCst), 1);
        })
    }

    #[test]
    fn test_upgrade_downgrade() {
        loom::model(|| {
            let m = IrqRwLock::new(());

            // An upgradeable handle may be taken alongside existing readers,
            // but blocks new readers, writers and other upgraders — and cannot
            // upgrade while a reader is still around.
            m.with_read_lock(|_| {
                m.try_with_upgradeable_read_lock(|mut u| {
                    assert!(m.try_with_read_lock(|_| ()).is_none());
                    assert!(m.try_with_write_lock(|_| ()).is_none());
                    assert!(u.try_upgrade(|_| ()).is_none());
                })
                .expect("upgradeable read may be taken alongside existing readers");
            });

            // A writer blocks upgraders.
            m.with_write_lock(|_| {
                assert!(m.try_with_upgradeable_read_lock(|_| ()).is_none());
            });

            m.with_upgradeable_read_lock(|u| {
                assert!(m.try_with_upgradeable_read_lock(|_| ()).is_none());

                // Downgrading hands off to a plain reader without ever
                // releasing the lock.
                u.downgrade(|_| {
                    assert!(m.try_with_read_lock(|_| ()).is_some());
                    assert!(m.try_with_write_lock(|_| ()).is_none());
                });
            });

            // With no readers around, the upgrade succeeds.
            m.with_upgradeable_read_lock(|mut u| {
                assert!(u.try_upgrade(|_| ()).is_some());
            });

            assert!(m.try_with_write_lock(|_| ()).is_some());
        })
    }

    #[test]
    fn concurrent_readers() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: IrqRwLock<()> = IrqRwLock::new(());
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        L.with_read_lock(|_| ());

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
    fn concurrent_writers() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: IrqRwLock<usize> = IrqRwLock::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        L.with_write_lock(|v| *v = v.wrapping_add(1));

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
    fn concurrent_readers_and_writer() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: IrqRwLock<()> = IrqRwLock::new(());
            }

            let mut threads = Vec::new();
            for i in 0..THREADS {
                threads.push(thread::spawn(move || {
                    for _ in 0..CYCLES {
                        if i == 0 {
                            L.with_write_lock(|_| ());
                        } else {
                            L.with_read_lock(|_| ());
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
    fn concurrent_exclusive_with_downgrade() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: IrqRwLock<usize> = IrqRwLock::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        // exclusive -> upgradeable -> shared, without ever
                        // releasing the lock in between.
                        L.with_upgradeable_read_lock(|mut u| {
                            u.upgrade(|v| *v = v.wrapping_add(1));
                            u.downgrade(|_| ());
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
    fn concurrent_upgrade() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: IrqRwLock<usize> = IrqRwLock::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        L.with_upgradeable_read_lock(|mut u| {
                            u.upgrade(|v| *v = v.wrapping_add(1));
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
    fn concurrent_upgradable_with_downgrade() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: IrqRwLock<()> = IrqRwLock::new(());
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        L.with_upgradeable_read_lock(|u| u.downgrade(|_| ()));

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
    fn concurrent_readers_and_upgrader() {
        loom::model(|| {
            loom::lazy_static! {
                static ref L: IrqRwLock<()> = IrqRwLock::new(());
            }

            let mut threads = Vec::new();
            for i in 0..THREADS {
                threads.push(thread::spawn(move || {
                    for _ in 0..CYCLES {
                        if i == 0 {
                            L.with_upgradeable_read_lock(|mut u| u.upgrade(|_| ()));
                        } else {
                            L.with_read_lock(|_| ());
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
}
