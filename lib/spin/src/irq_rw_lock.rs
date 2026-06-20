// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Implementation based on the `RwSpinLock` from Facebook's folly:
//! <https://github.com/facebook/folly/blob/main/folly/synchronization/RWSpinLock.h>

//! A lock that provides data access to either one writer or many readers.

use core::fmt;
use core::ops::{Deref, DerefMut};

use util::loom_const_fn;

use crate::util::{HeldInterrupts, hold_interrupts};
use crate::{RwLock, RwLockReadGuard, RwLockUpgradableGuard, RwLockWriteGuard};

/// A lock that provides data access to either one writer or many readers.
///
/// This lock behaves in a similar manner to its namesake `std::sync::RwLock` but uses
/// spinning for synchronisation instead. Unlike its namesake, this lock does not
/// track lock poisoning.
///
/// This type of lock allows a number of readers or at most one writer at any
/// point in time. The write portion of this lock typically allows modification
/// of the underlying data (exclusive access) and the read portion of this lock
/// typically allows for read-only access (shared access).
///
/// The type parameter `T` represents the data that this lock protects. It is
/// required that `T` satisfies `Send` to be shared across tasks and `Sync` to
/// allow concurrent access through readers. The RAII guards returned from the
/// locking methods implement `Deref` (and `DerefMut` for the `write` methods)
/// to allow access to the contained of the lock.
///
/// An [`RwLockUpgradableGuard`] can be upgraded to a writable guard through
/// the [`RwLockUpgradableGuard::upgrade`] and [`RwLockUpgradableGuard::try_upgrade`]
/// functions. Writable or upgradeable  guards can be downgraded through their
/// respective `downgrade` functions.
///
/// Based on Facebook's
/// [`folly/RWSpinLock.h`](https://github.com/facebook/folly/blob/a0394d84f2d5c3e50ebfd0566f9d3acb52cfab5a/folly/synchronization/RWSpinLock.h).
/// This implementation is unfair to writers - if the lock always has readers, then no writers will
/// ever get a chance. Using an upgradeable lock guard can *somewhat* alleviate this issue as no
/// new readers are allowed when an upgradeable guard is held, but upgradeable guards can be taken
/// when there are existing readers. However if the lock is that highly contended and writes are
/// crucial then this implementation may be a poor choice.
///
/// # Examples
///
/// ```
/// use spin;
///
/// let lock = spin::RwLock::new(5);
///
/// // many reader locks can be held at once
/// {
///     let r1 = lock.read();
///     let r2 = lock.read();
///     assert_eq!(*r1, 5);
///     assert_eq!(*r2, 5);
/// } // read locks are dropped at this point
///
/// // only one write lock may be held, however
/// {
///     let mut w = lock.write();
///     *w += 1;
///     assert_eq!(*w, 6);
/// } // write lock is dropped here
/// ```
pub struct IrqRwLock<T: ?Sized> {
    inner: RwLock<T>,
}

/// A guard that provides immutable data access.
///
/// When the guard falls out of scope it will decrement the read count,
/// potentially releasing the lock.
pub struct IrqRwLockReadGuard<'a, T: 'a + ?Sized> {
    guard: RwLockReadGuard<'a, T>,
    _held_irq: HeldInterrupts,
}

/// A guard that provides mutable data access.
///
/// When the guard falls out of scope it will release the lock.
pub struct IrqRwLockWriteGuard<'a, T: 'a + ?Sized> {
    guard: RwLockWriteGuard<'a, T>,
    _held_irq: HeldInterrupts,
}

/// A guard that provides immutable data access but can be upgraded to [`RwLockWriteGuard`].
///
/// No writers or other upgradeable guards can exist while this is in scope. New reader
/// creation is prevented (to alleviate writer starvation) but there may be existing readers
/// when the lock is acquired.
///
/// When the guard falls out of scope it will release the lock.
pub struct IrqRwLockUpgradableGuard<'a, T: 'a + ?Sized> {
    guard: RwLockUpgradableGuard<'a, T>,
    _held_irq: HeldInterrupts,
}

impl<T> IrqRwLock<T> {
    loom_const_fn! {
        /// Creates a new spinlock wrapping the supplied data.
        ///
        /// May be used statically:
        ///
        /// ```
        /// use spin;
        ///
        /// static RW_LOCK: spin::RwLock<()> = spin::RwLock::new(());
        ///
        /// fn demo() {
        ///     let lock = RW_LOCK.read();
        ///     // do something with lock
        ///     drop(lock);
        /// }
        /// ```
        #[inline]
        pub const fn new(data: T) -> Self {
            IrqRwLock {
                inner: RwLock::new(data)
            }
        }
    }

    /// Consumes this `RwLock`, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        // We know statically that there are no outstanding references to
        // `self` so there's no need to lock.
        let IrqRwLock { inner, .. } = self;
        inner.into_inner()
    }
}

impl<T: ?Sized> IrqRwLock<T> {
    /// Locks this rwlock with shared read access, blocking the current thread
    /// until it can be acquired.
    ///
    /// The calling thread will be blocked until there are no more writers which
    /// hold the lock. There may be other readers currently inside the lock when
    /// this method returns. This method does not provide any guarantees with
    /// respect to the ordering of whether contentious readers or writers will
    /// acquire the lock first.
    ///
    /// Returns an RAII guard which will release this thread's shared access
    /// once it is dropped.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// {
    ///     let mut data = mylock.read();
    ///     // The lock is now locked and the data can be read
    ///     println!("{}", *data);
    ///     // The lock is dropped
    /// }
    /// ```
    #[inline]
    pub fn read(&self) -> IrqRwLockReadGuard<'_, T> {
        // Disable IRQs first, THEN acquire the spinlock.
        // Reversing the order would leave a window where the ISR fires after
        // the spinlock is acquired but before IRQs are masked => deadlock.
        let _held_irq = hold_interrupts();

        IrqRwLockReadGuard {
            guard: self.inner.read(),
            _held_irq,
        }
    }

    /// Lock this rwlock with exclusive write access, blocking the current
    /// thread until it can be acquired.
    ///
    /// This function will not return while other writers or other readers
    /// currently have access to the lock.
    ///
    /// Returns an RAII guard which will drop the write access of this rwlock
    /// when dropped.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// {
    ///     let mut data = mylock.write();
    ///     // The lock is now locked and the data can be written
    ///     *data += 1;
    ///     // The lock is dropped
    /// }
    /// ```
    #[inline]
    pub fn write(&self) -> IrqRwLockWriteGuard<'_, T> {
        // Disable IRQs first, THEN acquire the spinlock.
        // Reversing the order would leave a window where the ISR fires after
        // the spinlock is acquired but before IRQs are masked => deadlock.
        let _held_irq = hold_interrupts();

        IrqRwLockWriteGuard {
            guard: self.inner.write(),
            _held_irq,
        }
    }

    /// Obtain a readable lock guard that can later be upgraded to a writable lock guard.
    /// Upgrades can be done through the [`RwLockUpgradableGuard::upgrade`](RwLockUpgradableGuard::upgrade) method.
    #[inline]
    pub fn upgradeable_read(&self) -> IrqRwLockUpgradableGuard<'_, T> {
        // Disable IRQs first, THEN acquire the spinlock.
        // Reversing the order would leave a window where the ISR fires after
        // the spinlock is acquired but before IRQs are masked => deadlock.
        let _held_irq = hold_interrupts();

        IrqRwLockUpgradableGuard {
            guard: self.inner.upgradeable_read(),
            _held_irq,
        }
    }

    /// Attempt to acquire this lock with shared read access.
    ///
    /// This function will never block and will return immediately if `read`
    /// would otherwise succeed. Returns `Some` of an RAII guard which will
    /// release the shared access of this thread when dropped, or `None` if the
    /// access could not be granted. This method does not provide any
    /// guarantees with respect to the ordering of whether contentious readers
    /// or writers will acquire the lock first.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// {
    ///     match mylock.try_read() {
    ///         Some(data) => {
    ///             // The lock is now locked and the data can be read
    ///             println!("{}", *data);
    ///             // The lock is dropped
    ///         },
    ///         None => (), // no cigar
    ///     };
    /// }
    /// ```
    #[inline]
    pub fn try_read(&self) -> Option<IrqRwLockReadGuard<'_, T>> {
        let _held_irq = hold_interrupts();

        // If the lock is taken, `_held_irq` is dropped here, restoring IRQs;
        // otherwise it moves into the guard and restores them on unlock.
        self.inner
            .try_read()
            .map(|guard| IrqRwLockReadGuard { guard, _held_irq })
    }

    /// Attempt to lock this rwlock with exclusive write access.
    ///
    /// This function does not ever block, and it will return `None` if a call
    /// to `write` would otherwise block. If successful, an RAII guard is
    /// returned.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// {
    ///     match mylock.try_write() {
    ///         Some(mut data) => {
    ///             // The lock is now locked and the data can be written
    ///             *data += 1;
    ///             // The lock is implicitly dropped
    ///         },
    ///         None => (), // no cigar
    ///     };
    /// }
    /// ```
    #[inline]
    pub fn try_write(&self) -> Option<IrqRwLockWriteGuard<'_, T>> {
        let _held_irq = hold_interrupts();

        // If the lock is taken, `_held_irq` is dropped here, restoring IRQs;
        // otherwise it moves into the guard and restores them on unlock.
        self.inner
            .try_write()
            .map(|guard| IrqRwLockWriteGuard { guard, _held_irq })
    }

    /// Attempt to lock this rwlock with exclusive write access.
    ///
    /// Unlike [`RwLock::try_write`], this function is allowed to spuriously fail even when acquiring exclusive write access
    /// would otherwise succeed, which can result in more efficient code on some platforms.
    #[inline]
    pub fn try_write_weak(&self) -> Option<IrqRwLockWriteGuard<'_, T>> {
        let _held_irq = hold_interrupts();

        // If the lock is taken, `_held_irq` is dropped here, restoring IRQs;
        // otherwise it moves into the guard and restores them on unlock.
        self.inner
            .try_write_weak()
            .map(|guard| IrqRwLockWriteGuard { guard, _held_irq })
    }

    /// Tries to obtain an upgradeable lock guard.
    #[inline]
    pub fn try_upgradeable_read(&self) -> Option<IrqRwLockUpgradableGuard<'_, T>> {
        let _held_irq = hold_interrupts();

        // If the lock is taken, `_held_irq` is dropped here, restoring IRQs;
        // otherwise it moves into the guard and restores them on unlock.
        self.inner
            .try_upgradeable_read()
            .map(|guard| IrqRwLockUpgradableGuard { guard, _held_irq })
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
    /// Because [`RwLock`] guarantees exclusive mutable access, this function may only return either `0` or `1`.
    ///
    /// # Safety
    ///
    /// This function provides no synchronization guarantees and so its result should be considered 'out of date'
    /// the instant it is called. Do not use it for synchronization purposes. However, it may be useful as a heuristic.
    pub fn writer_count(&self) -> usize {
        self.inner.writer_count()
    }

    /// Calls s given callback with a mutable reference to the underlying data.
    ///
    /// Since this call borrows the `RwLock` mutably, no actual locking needs to
    /// take place -- the mutable borrow statically guarantees no locks exist.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut lock = spin::RwLock::new(0);
    /// *lock.get_mut() = 10;
    /// assert_eq!(*lock.read(), 10);
    /// ```
    #[inline(always)]
    pub fn with_mut<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(*mut T) -> R,
    {
        self.inner.with_mut(f)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for IrqRwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_read() {
            Some(guard) => write!(f, "IrqRwLock {{ data: ")
                .and_then(|()| guard.fmt(f))
                .and_then(|()| write!(f, " }}")),
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

impl<T: ?Sized> Deref for IrqRwLockReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.guard.deref()
    }
}

impl<'rwlock, T: ?Sized + fmt::Debug> fmt::Debug for IrqRwLockReadGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized + fmt::Display> fmt::Display for IrqRwLockReadGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized> IrqRwLockUpgradableGuard<'rwlock, T> {
    /// Upgrades an upgradeable lock guard to a writable lock guard.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    ///
    /// let upgradeable = mylock.upgradeable_read(); // Readable, but not yet writable
    /// let writable = upgradeable.upgrade();
    /// ```
    #[inline]
    pub fn upgrade(self) -> IrqRwLockWriteGuard<'rwlock, T> {
        let guard = self.guard.upgrade();

        IrqRwLockWriteGuard {
            guard,
            _held_irq: self._held_irq,
        }
    }

    /// Tries to upgrade an upgradeable lock guard to a writable lock guard.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// let upgradeable = mylock.upgradeable_read(); // Readable, but not yet writable
    ///
    /// match upgradeable.try_upgrade() {
    ///     Ok(writable) => /* upgrade successful - use writable lock guard */ (),
    ///     Err(upgradeable) => /* upgrade unsuccessful */ (),
    /// };
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `Err(self)`, with the upgradable guard returned unchanged, if any other readers
    /// are currently holding the lock.
    #[inline]
    pub fn try_upgrade(self) -> Result<IrqRwLockWriteGuard<'rwlock, T>, Self> {
        match self.guard.try_upgrade() {
            Ok(guard) => Ok(IrqRwLockWriteGuard {
                guard,
                _held_irq: self._held_irq,
            }),
            Err(guard) => Err(IrqRwLockUpgradableGuard {
                guard,
                _held_irq: self._held_irq,
            }),
        }
    }

    /// Tries to upgrade an upgradeable lock guard to a writable lock guard.
    ///
    /// Unlike [`RwLockUpgradableGuard::try_upgrade`], this function is allowed to spuriously fail even when upgrading
    /// would otherwise succeed, which can result in more efficient code on some platforms.
    ///
    /// # Errors
    ///
    /// Returns `Err(self)`, with the upgradable guard returned unchanged, if either:
    /// - other readers are currently holding the lock, or
    /// - the underlying compare-exchange spuriously failed (allowed by this variant — see above).
    ///
    /// For the non-spurious variant, see [`Self::try_upgrade`].
    #[inline]
    pub fn try_upgrade_weak(self) -> Result<IrqRwLockWriteGuard<'rwlock, T>, Self> {
        match self.guard.try_upgrade_weak() {
            Ok(guard) => Ok(IrqRwLockWriteGuard {
                guard,
                _held_irq: self._held_irq,
            }),
            Err(guard) => Err(IrqRwLockUpgradableGuard {
                guard,
                _held_irq: self._held_irq,
            }),
        }
    }

    #[inline]
    /// Downgrades the upgradeable lock guard to a readable, shared lock guard. Cannot fail and is guaranteed not to spin.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(1);
    ///
    /// let upgradeable = mylock.upgradeable_read();
    /// assert!(mylock.try_read().is_none());
    /// assert_eq!(*upgradeable, 1);
    ///
    /// let readable = upgradeable.downgrade(); // This is guaranteed not to spin
    /// assert!(mylock.try_read().is_some());
    /// assert_eq!(*readable, 1);
    /// ```
    pub fn downgrade(self) -> IrqRwLockReadGuard<'rwlock, T> {
        IrqRwLockReadGuard {
            guard: self.guard.downgrade(),
            _held_irq: self._held_irq,
        }
    }
}

impl<T: ?Sized> Deref for IrqRwLockUpgradableGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.guard.deref()
    }
}

impl<'rwlock, T: ?Sized + fmt::Debug> fmt::Debug for IrqRwLockUpgradableGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized + fmt::Display> fmt::Display for IrqRwLockUpgradableGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized> IrqRwLockWriteGuard<'rwlock, T> {
    /// Downgrades the writable lock guard to a readable, shared lock guard. Cannot fail and is guaranteed not to spin.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    ///
    /// let mut writable = mylock.write();
    /// *writable = 1;
    ///
    /// let readable = writable.downgrade(); // This is guaranteed not to spin
    /// # let readable_2 = mylock.try_read().unwrap();
    /// assert_eq!(*readable, 1);
    /// ```
    #[inline]
    pub fn downgrade(self) -> IrqRwLockReadGuard<'rwlock, T> {
        IrqRwLockReadGuard {
            guard: self.guard.downgrade(),
            _held_irq: self._held_irq,
        }
    }

    /// Downgrades the writable lock guard to an upgradable, shared lock guard. Cannot fail and is guaranteed not to spin.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    ///
    /// let mut writable = mylock.write();
    /// *writable = 1;
    ///
    /// let readable = writable.downgrade_to_upgradeable(); // This is guaranteed not to spin
    /// assert_eq!(*readable, 1);
    /// ```
    #[inline]
    pub fn downgrade_to_upgradeable(self) -> IrqRwLockUpgradableGuard<'rwlock, T> {
        IrqRwLockUpgradableGuard {
            guard: self.guard.downgrade_to_upgradeable(),
            _held_irq: self._held_irq,
        }
    }
}

impl<T: ?Sized> Deref for IrqRwLockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.guard.deref()
    }
}

impl<T: ?Sized> DerefMut for IrqRwLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.guard.deref_mut()
    }
}

impl<'rwlock, T: ?Sized + fmt::Debug> fmt::Debug for IrqRwLockWriteGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized + fmt::Display> fmt::Display for IrqRwLockWriteGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

#[cfg(test)]
mod tests {
    use core::mem;

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
            drop(l.read());
            drop(l.write());
            drop((l.read(), l.read()));
            drop(l.write());
        });
    }

    #[test]
    fn test_rwlock_unsized() {
        loom::model(|| {
            let rw: &IrqRwLock<[i32]> = &IrqRwLock::new([1, 2, 3]);
            {
                let b = &mut *rw.write();
                b[0] = 4;
                b[2] = 5;
            }
            let comp: &[i32] = &[4, 2, 5];
            assert_eq!(&*rw.read(), comp);
        })
    }

    #[test]
    fn test_rwlock_try_write() {
        loom::model(|| {
            use std::mem::drop;

            let lock = IrqRwLock::new(0isize);
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
        })
    }

    // #[test]
    // fn test_rw_try_read() {
    //     loom::model(|| {
    //         let m = IrqRwLock::new(0);
    //         mem::forget(m.write());
    //         assert!(m.try_read().is_none());
    //     })
    // }

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
            {
                let _r = m.read();
                let upg = m.try_upgradeable_read().unwrap();
                assert!(m.try_read().is_none());
                assert!(m.try_write().is_none());
                assert!(upg.try_upgrade().is_err());
            }
            {
                let w = m.write();
                assert!(m.try_upgradeable_read().is_none());
                let _r = w.downgrade();
                assert!(m.try_upgradeable_read().is_some());
                assert!(m.try_read().is_some());
                assert!(m.try_write().is_none());
            }
            {
                let _u = m.upgradeable_read();
                assert!(m.try_upgradeable_read().is_none());
            }

            assert!(m.try_upgradeable_read().unwrap().try_upgrade().is_ok());
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
                        let g = L.read();
                        drop(g);

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
                        let mut g = L.write();
                        *g = g.wrapping_add(1);
                        drop(g);

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
                            drop(L.write());
                        } else {
                            drop(L.read());
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
                        let mut g = L.write();
                        *g = g.wrapping_add(1);
                        let g = IrqRwLockWriteGuard::downgrade_to_upgradeable(g);
                        let g = IrqRwLockUpgradableGuard::downgrade(g);
                        drop(g);

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
                        let g = L.upgradeable_read();
                        let mut g = IrqRwLockUpgradableGuard::upgrade(g);
                        *g = g.wrapping_add(1);
                        drop(g);

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
                        let g = L.upgradeable_read();
                        let g = IrqRwLockUpgradableGuard::downgrade(g);
                        drop(g);

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
                            let g = L.upgradeable_read();
                            let g = IrqRwLockUpgradableGuard::upgrade(g);
                            drop(g);
                        } else {
                            drop(L.read());
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
