// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Implementation based on the `RwSpinLock` from Facebook's folly:
//! <https://github.com/facebook/folly/blob/main/folly/synchronization/RWSpinLock.h>

//! A lock that provides data access to either one writer or many readers.

use core::ops::{Deref, DerefMut};
use core::{fmt, mem};

use util::loom_const_fn;

use crate::Backoff;
use crate::loom::cell::UnsafeCell;
use crate::loom::sync::atomic::{AtomicUsize, Ordering};

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
pub struct RwLock<T: ?Sized> {
    lock: AtomicUsize,
    data: UnsafeCell<T>,
}

const READER: usize = 1 << 2;
const UPGRADED: usize = 1 << 1;
const WRITER: usize = 1;

/// A guard that provides immutable data access.
///
/// When the guard falls out of scope it will decrement the read count,
/// potentially releasing the lock.
pub struct RwLockReadGuard<'a, T: 'a + ?Sized> {
    inner: &'a RwLock<T>,
}

/// A guard that provides mutable data access.
///
/// When the guard falls out of scope it will release the lock.
pub struct RwLockWriteGuard<'a, T: 'a + ?Sized> {
    inner: &'a RwLock<T>,
}

/// A guard that provides immutable data access but can be upgraded to [`RwLockWriteGuard`].
///
/// No writers or other upgradeable guards can exist while this is in scope. New reader
/// creation is prevented (to alleviate writer starvation) but there may be existing readers
/// when the lock is acquired.
///
/// When the guard falls out of scope it will release the lock.
pub struct RwLockUpgradableGuard<'a, T: 'a + ?Sized> {
    inner: &'a RwLock<T>,
}

// Same unsafe impls as `std::sync::RwLock`
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

unsafe impl<T: ?Sized + Send + Sync> Send for RwLockWriteGuard<'_, T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLockWriteGuard<'_, T> {}

unsafe impl<T: ?Sized + Sync> Send for RwLockReadGuard<'_, T> {}
unsafe impl<T: ?Sized + Sync> Sync for RwLockReadGuard<'_, T> {}

unsafe impl<T: ?Sized + Send + Sync> Send for RwLockUpgradableGuard<'_, T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLockUpgradableGuard<'_, T> {}

impl<T> RwLock<T> {
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
            RwLock {
                lock: AtomicUsize::new(0),
                data: UnsafeCell::new(data),
            }
        }
    }

    /// Consumes this `RwLock`, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        // We know statically that there are no outstanding references to
        // `self` so there's no need to lock.
        let RwLock { data, .. } = self;
        data.into_inner()
    }
}

impl<T: ?Sized> RwLock<T> {
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
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        let mut boff = Backoff::new();
        loop {
            match self.try_read() {
                Some(guard) => return guard,
                None => boff.spin(),
            }
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
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        let mut boff = Backoff::new();
        loop {
            match self.try_write_internal(false) {
                Some(guard) => return guard,
                None => boff.spin(),
            }
        }
    }

    /// Obtain a readable lock guard that can later be upgraded to a writable lock guard.
    /// Upgrades can be done through the [`RwLockUpgradableGuard::upgrade`](RwLockUpgradableGuard::upgrade) method.
    #[inline]
    pub fn upgradeable_read(&self) -> RwLockUpgradableGuard<'_, T> {
        let mut boff = Backoff::new();
        loop {
            match self.try_upgradeable_read() {
                Some(guard) => return guard,
                None => boff.spin(),
            }
        }
    }
}

impl<T: ?Sized> RwLock<T> {
    // Acquire a read lock, returning the new lock value.
    fn acquire_reader(&self) -> usize {
        // An arbitrary cap that allows us to catch overflows long before they happen
        const MAX_READERS: usize = core::usize::MAX / READER / 2;

        let value = self.lock.fetch_add(READER, Ordering::Acquire);

        if value > MAX_READERS * READER {
            self.lock.fetch_sub(READER, Ordering::Relaxed);
            panic!("Too many lock readers, cannot safely proceed");
        } else {
            value
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
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let value = self.acquire_reader();

        // We check the UPGRADED bit here so that new readers are prevented when an UPGRADED lock is held.
        // This helps reduce writer starvation.
        if value & (WRITER | UPGRADED) != 0 {
            // Lock is taken, undo.
            self.lock.fetch_sub(READER, Ordering::Release);
            None
        } else {
            Some(RwLockReadGuard { inner: self })
        }
    }

    /// Returns `true` if the lock is currently held in any mode.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed) != 0
    }

    /// Returns `true` if the lock is held in exclusive mode.
    #[inline]
    pub fn is_locked_exclusive(&self) -> bool {
        self.lock.load(Ordering::Relaxed) & WRITER != 0
    }

    /// Return the number of readers that currently hold the lock (including upgradable readers).
    ///
    /// # Safety
    ///
    /// This function provides no synchronization guarantees and so its result should be considered 'out of date'
    /// the instant it is called. Do not use it for synchronization purposes. However, it may be useful as a heuristic.
    pub fn reader_count(&self) -> usize {
        let state = self.lock.load(Ordering::Relaxed);
        state / READER + (state & UPGRADED) / UPGRADED
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
        (self.lock.load(Ordering::Relaxed) & WRITER) / WRITER
    }

    /// Force decrement the reader count.
    ///
    /// # Safety
    ///
    /// This is *extremely* unsafe if there are outstanding `RwLockReadGuard`s
    /// live, or if called more times than `read` has been called, but can be
    /// useful in FFI contexts where the caller doesn't know how to deal with
    /// RAII. The underlying atomic operation uses `Ordering::Release`.
    #[inline]
    pub unsafe fn force_read_decrement(&self) {
        debug_assert!(self.lock.load(Ordering::Relaxed) & !WRITER > 0);
        self.lock.fetch_sub(READER, Ordering::Release);
    }

    /// Force unlock exclusive write access.
    ///
    /// # Safety
    ///
    /// This is *extremely* unsafe if there are outstanding `RwLockWriteGuard`s
    /// live, or if called when there are current readers, but can be useful in
    /// FFI contexts where the caller doesn't know how to deal with RAII. The
    /// underlying atomic operation uses `Ordering::Release`.
    #[inline]
    pub unsafe fn force_write_unlock(&self) {
        debug_assert_eq!(self.lock.load(Ordering::Relaxed) & !(WRITER | UPGRADED), 0);
        self.lock.fetch_and(!(WRITER | UPGRADED), Ordering::Release);
    }

    #[inline(always)]
    fn try_write_internal(&self, strong: bool) -> Option<RwLockWriteGuard<'_, T>> {
        if compare_exchange(
            &self.lock,
            0,
            WRITER,
            Ordering::Acquire,
            Ordering::Relaxed,
            strong,
        )
        .is_ok()
        {
            Some(RwLockWriteGuard { inner: self })
        } else {
            None
        }
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
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        self.try_write_internal(true)
    }

    /// Attempt to lock this rwlock with exclusive write access.
    ///
    /// Unlike [`RwLock::try_write`], this function is allowed to spuriously fail even when acquiring exclusive write access
    /// would otherwise succeed, which can result in more efficient code on some platforms.
    #[inline]
    pub fn try_write_weak(&self) -> Option<RwLockWriteGuard<'_, T>> {
        self.try_write_internal(false)
    }

    /// Tries to obtain an upgradeable lock guard.
    #[inline]
    pub fn try_upgradeable_read(&self) -> Option<RwLockUpgradableGuard<'_, T>> {
        if self.lock.fetch_or(UPGRADED, Ordering::Acquire) & (WRITER | UPGRADED) == 0 {
            Some(RwLockUpgradableGuard { inner: self })
        } else {
            // We can't unflip the UPGRADED bit back just yet as there is another upgradeable or write lock.
            // When they unlock, they will clear the bit.
            None
        }
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
        self.data.with_mut(f)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_read() {
            Some(guard) => write!(f, "RwLock {{ data: ")
                .and_then(|()| guard.fmt(f))
                .and_then(|()| write!(f, " }}")),
            None => write!(f, "RwLock {{ <locked> }}"),
        }
    }
}

impl<T: ?Sized + Default> Default for RwLock<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T> From<T> for RwLock<T> {
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // Safety: the read guard guarantees shared read access.
        self.inner.data.with(|ptr| unsafe { &*ptr })
    }
}

impl<'rwlock, T: ?Sized + fmt::Debug> fmt::Debug for RwLockReadGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized + fmt::Display> fmt::Display for RwLockReadGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized> RwLockUpgradableGuard<'rwlock, T> {
    /// Upgrades an upgradeable lock guard to a writable lock guard.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    ///
    /// let upgradeable = mylock.upgradeable_read(); // Readable, but not yet writable
    /// let writable = upgradeable.upgrade();
    /// ```
    #[inline]
    pub fn upgrade(mut self) -> RwLockWriteGuard<'rwlock, T> {
        let mut boff = Backoff::new();
        loop {
            self = match self.try_upgrade_internal(false) {
                Ok(guard) => return guard,
                Err(e) => e,
            };

            boff.spin();
        }
    }

    #[inline(always)]
    fn try_upgrade_internal(self, strong: bool) -> Result<RwLockWriteGuard<'rwlock, T>, Self> {
        if compare_exchange(
            &self.inner.lock,
            UPGRADED,
            WRITER,
            Ordering::Acquire,
            Ordering::Relaxed,
            strong,
        )
        .is_ok()
        {
            let inner = self.inner;

            // Forget the old guard so its destructor doesn't run (before mutably aliasing data below)
            mem::forget(self);

            // Upgrade successful
            Ok(RwLockWriteGuard { inner })
        } else {
            Err(self)
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
    #[inline]
    pub fn try_upgrade(self) -> Result<RwLockWriteGuard<'rwlock, T>, Self> {
        self.try_upgrade_internal(true)
    }

    /// Tries to upgrade an upgradeable lock guard to a writable lock guard.
    ///
    /// Unlike [`RwLockUpgradableGuard::try_upgrade`], this function is allowed to spuriously fail even when upgrading
    /// would otherwise succeed, which can result in more efficient code on some platforms.
    #[inline]
    pub fn try_upgrade_weak(self) -> Result<RwLockWriteGuard<'rwlock, T>, Self> {
        self.try_upgrade_internal(false)
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
    pub fn downgrade(self) -> RwLockReadGuard<'rwlock, T> {
        // Reserve the read guard for ourselves
        self.inner.acquire_reader();

        let inner = self.inner;

        // Dropping self removes the UPGRADED bit
        mem::drop(self);

        RwLockReadGuard { inner }
    }
}

impl<T: ?Sized> Deref for RwLockUpgradableGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // Safety: the upgradable guard guarantees shared read access.
        self.inner.data.with(|ptr| unsafe { &*ptr })
    }
}

impl<'rwlock, T: ?Sized + fmt::Debug> fmt::Debug for RwLockUpgradableGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized + fmt::Display> fmt::Display for RwLockUpgradableGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized> RwLockWriteGuard<'rwlock, T> {
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
    pub fn downgrade(self) -> RwLockReadGuard<'rwlock, T> {
        // Reserve the read guard for ourselves
        self.inner.acquire_reader();

        let inner = self.inner;

        // Dropping self removes the UPGRADED bit
        mem::drop(self);

        RwLockReadGuard { inner }
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
    pub fn downgrade_to_upgradeable(self) -> RwLockUpgradableGuard<'rwlock, T> {
        debug_assert_eq!(
            self.inner.lock.load(Ordering::Acquire) & (WRITER | UPGRADED),
            WRITER
        );

        // Reserve the read guard for ourselves
        self.inner.lock.store(UPGRADED, Ordering::Release);

        let inner = self.inner;

        // Dropping self removes the UPGRADED bit
        mem::forget(self);

        RwLockUpgradableGuard { inner }
    }
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // Safety: the write guard holds exclusive access.
        self.inner.data.with(|ptr| unsafe { &*ptr })
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // Safety: the write guard holds exclusive access.
        self.inner.data.with_mut(|ptr| unsafe { &mut *ptr })
    }
}

impl<'rwlock, T: ?Sized + fmt::Debug> fmt::Debug for RwLockWriteGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized + fmt::Display> fmt::Display for RwLockWriteGuard<'rwlock, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<'rwlock, T: ?Sized> Drop for RwLockReadGuard<'rwlock, T> {
    fn drop(&mut self) {
        debug_assert!(self.inner.lock.load(Ordering::Relaxed) & !(WRITER | UPGRADED) > 0);
        self.inner.lock.fetch_sub(READER, Ordering::Release);
    }
}

impl<'rwlock, T: ?Sized> Drop for RwLockUpgradableGuard<'rwlock, T> {
    fn drop(&mut self) {
        debug_assert_eq!(
            self.inner.lock.load(Ordering::Relaxed) & (WRITER | UPGRADED),
            UPGRADED
        );
        self.inner.lock.fetch_sub(UPGRADED, Ordering::AcqRel);
    }
}

impl<'rwlock, T: ?Sized> Drop for RwLockWriteGuard<'rwlock, T> {
    fn drop(&mut self) {
        debug_assert_eq!(self.inner.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        self.inner
            .lock
            .fetch_and(!(WRITER | UPGRADED), Ordering::Release);
    }
}

#[inline(always)]
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
    use crate::loom;
    use crate::loom::sync::Arc;
    use crate::loom::sync::atomic::{AtomicUsize, Ordering};
    use crate::loom::thread;

    /// Threads to spawn in concurrency tests. Loom's model checker explores
    /// every possible interleaving, so the state space scales exponentially
    /// with thread count. Two threads are enough to exercise every race a
    /// rwlock can produce.
    const THREADS: usize = if cfg!(loom) { 2 } else { loom::MAX_THREADS - 1 };

    /// Number of cycles to repeat concurrency tests for.
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

    #[derive(Eq, PartialEq, Debug)]
    struct NonCopy(i32);

    #[test]
    fn smoke() {
        loom::model(|| {
            let l = RwLock::new(());
            drop(l.read());
            drop(l.write());
            drop((l.read(), l.read()));
            drop(l.write());
        });
    }

    #[test]
    fn test_rwlock_unsized() {
        loom::model(|| {
            let rw: &RwLock<[i32]> = &RwLock::new([1, 2, 3]);
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
        })
    }

    #[test]
    fn test_rw_try_read() {
        loom::model(|| {
            let m = RwLock::new(0);
            mem::forget(m.write());
            assert!(m.try_read().is_none());
        })
    }

    #[test]
    fn test_into_inner() {
        loom::model(|| {
            let m = RwLock::new(NonCopy(10));
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
            let m = RwLock::new(Foo(num_drops.clone()));
            assert_eq!(num_drops.load(Ordering::SeqCst), 0);
            {
                let _inner = m.into_inner();
                assert_eq!(num_drops.load(Ordering::SeqCst), 0);
            }
            assert_eq!(num_drops.load(Ordering::SeqCst), 1);
        })
    }

    #[test]
    fn test_force_read_decrement() {
        loom::model(|| {
            let m = RwLock::new(());
            mem::forget(m.read());
            mem::forget(m.read());
            mem::forget(m.read());
            assert!(m.try_write().is_none());
            unsafe {
                m.force_read_decrement();
                m.force_read_decrement();
            }
            assert!(m.try_write().is_none());
            unsafe {
                m.force_read_decrement();
            }
            assert!(m.try_write().is_some());
        })
    }

    #[test]
    fn test_force_write_unlock() {
        loom::model(|| {
            let m = RwLock::new(());
            mem::forget(m.write());
            assert!(m.try_read().is_none());
            unsafe {
                m.force_write_unlock();
            }
            assert!(m.try_read().is_some());
        })
    }

    #[test]
    fn test_upgrade_downgrade() {
        loom::model(|| {
            let m = RwLock::new(());
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
                static ref L: RwLock<()> = RwLock::new(());
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
                static ref L: RwLock<usize> = RwLock::new(0);
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
                static ref L: RwLock<()> = RwLock::new(());
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
                static ref L: RwLock<usize> = RwLock::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let mut g = L.write();
                        *g = g.wrapping_add(1);
                        let g = RwLockWriteGuard::downgrade_to_upgradeable(g);
                        let g = RwLockUpgradableGuard::downgrade(g);
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
                static ref L: RwLock<usize> = RwLock::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let g = L.upgradeable_read();
                        let mut g = RwLockUpgradableGuard::upgrade(g);
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
                static ref L: RwLock<()> = RwLock::new(());
            }

            let mut threads = Vec::new();
            for _ in 0..THREADS {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let g = L.upgradeable_read();
                        let g = RwLockUpgradableGuard::downgrade(g);
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
                static ref L: RwLock<()> = RwLock::new(());
            }

            let mut threads = Vec::new();
            for i in 0..THREADS {
                threads.push(thread::spawn(move || {
                    for _ in 0..CYCLES {
                        if i == 0 {
                            let g = L.upgradeable_read();
                            let g = RwLockUpgradableGuard::upgrade(g);
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
