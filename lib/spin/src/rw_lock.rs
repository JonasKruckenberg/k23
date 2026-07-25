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
use core::ops::Deref;

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
/// allow concurrent access through readers.
///
/// [`RwLock::with_upgradeable_read`] hands its closure an [`Upgradeable`]
/// handle that can be temporarily upgraded to exclusive access via
/// [`Upgradeable::upgrade`], or handed off to shared access via
/// [`Upgradeable::downgrade`].
///
/// Based on Facebook's
/// [`folly/RWSpinLock.h`](https://github.com/facebook/folly/blob/a0394d84f2d5c3e50ebfd0566f9d3acb52cfab5a/folly/synchronization/RWSpinLock.h).
/// This implementation is unfair to writers - if the lock always has readers, then no writers will
/// ever get a chance. Using an upgradeable lock can *somewhat* alleviate this issue as no
/// new readers are allowed when an upgradeable handle is held, but upgradeable handles can be taken
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
pub struct RwLock<T: ?Sized> {
    lock: AtomicUsize,
    data: UnsafeCell<T>,
}

const READER: usize = 1 << 2;
const UPGRADED: usize = 1 << 1;
const WRITER: usize = 1;

/// Shared access to an [`RwLock`] that can be upgraded to exclusive access.
///
/// Handed to the closure passed to [`RwLock::with_upgradeable_read`] and
/// released when that closure returns. No writers or other upgradeable handles
/// can exist while this is alive. New reader creation is prevented (to
/// alleviate writer starvation) but there may be existing readers when the lock
/// is acquired.
pub struct Upgradeable<'a, T: 'a + ?Sized> {
    lock: &'a RwLock<T>,
}

// Safety: spinlocks can be unlocked from any thread, therefore `RwLock` is `Send` as long as `T` is `Send`.
unsafe impl<T: ?Sized + Send> Send for RwLock<T> {}
// Safety: `RwLock` (by design) can be read from multiple threads which requires `T: Sync`.
// It also (again by design) hands out `&mut T` so other threads _could_ `mem::take` the value.
// This means `T` must be safe to move to a different thread, hence `T: Send`.
unsafe impl<T: ?Sized + Send + Sync> Sync for RwLock<T> {}

// Safety: sending an `Upgradeable` to another thread gives that thread `&T` directly
// (`T: Sync`) and the ability to upgrade to `&mut T` (`T: Send`).
unsafe impl<T: ?Sized + Send + Sync> Send for Upgradeable<'_, T> {}
// Safety: `&Upgradeable` derefs to `&T` (`T: Sync`), and because the handle can be upgraded
// to exclusive access it inherits the upgrade requirement of `T: Send`.
unsafe impl<T: ?Sized + Send + Sync> Sync for Upgradeable<'_, T> {}

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
        ///     RW_LOCK.with_read_lock(|lock| {
        ///         // do something with lock
        ///     });
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
    /// until it can be acquired, and calls `f` with shared access to the
    /// protected data.
    ///
    /// The calling thread will be blocked until there are no more writers which
    /// hold the lock. There may be other readers currently inside the lock when
    /// `f` runs. This method does not provide any guarantees with respect to
    /// the ordering of whether contentious readers or writers will acquire the
    /// lock first.
    ///
    /// The lock is released once `f` returns, including when `f` unwinds.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// mylock.with_read_lock(|data| {
    ///     // The lock is now locked and the data can be read
    ///     println!("{}", *data);
    /// }); // The lock is released here
    /// ```
    #[inline]
    pub fn with_read_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let mut boff = Backoff::new();
        while !self.raw_try_read() {
            boff.spin();
        }
        let _release = ReleaseRead(self);

        // Safety: this thread holds a read reference, so no writer can be active.
        self.data.with(|ptr| f(unsafe { &*ptr }))
    }

    /// Attempts to acquire this lock with shared read access and, on success,
    /// calls `f` with shared access to the protected data.
    ///
    /// This function will never block and will return `None` without running
    /// `f` if [`RwLock::with_read`] would otherwise block. This method does not
    /// provide any guarantees with respect to the ordering of whether
    /// contentious readers or writers will acquire the lock first.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// match mylock.try_with_read_lock(|data| println!("{}", *data)) {
    ///     Some(()) => (), // The lock was taken and has been released again
    ///     None => (),     // no cigar
    /// };
    /// ```
    #[inline]
    pub fn try_with_read_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&T) -> R,
    {
        if !self.raw_try_read() {
            return None;
        }
        let _release = ReleaseRead(self);

        // Safety: this thread holds a read reference, so no writer can be active.
        Some(self.data.with(|ptr| f(unsafe { &*ptr })))
    }

    /// Locks this rwlock with exclusive write access, blocking the current
    /// thread until it can be acquired, and calls `f` with exclusive access to
    /// the protected data.
    ///
    /// This function will not run `f` while other writers or other readers
    /// currently have access to the lock.
    ///
    /// The lock is released once `f` returns, including when `f` unwinds.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// mylock.with_write_lock(|data| {
    ///     // The lock is now locked and the data can be written
    ///     *data += 1;
    /// }); // The lock is released here
    /// ```
    #[inline]
    pub fn with_write_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let mut boff = Backoff::new();
        while !self.raw_try_write(false) {
            boff.spin();
        }
        let _release = ReleaseWrite(self);

        // Safety: the WRITER bit is held, so this thread has exclusive access.
        self.data.with_mut(|ptr| f(unsafe { &mut *ptr }))
    }

    /// Attempts to lock this rwlock with exclusive write access and, on
    /// success, calls `f` with exclusive access to the protected data.
    ///
    /// This function does not ever block, and it will return `None` without
    /// running `f` if a call to [`RwLock::with_write`] would otherwise block.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    /// match mylock.try_with_write_lock(|data| *data += 1) {
    ///     Some(()) => (), // The lock was taken and has been released again
    ///     None => (),     // no cigar
    /// };
    /// ```
    #[inline]
    pub fn try_with_write_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        self.try_with_write_internal(true, f)
    }

    /// Attempts to lock this rwlock with exclusive write access and, on
    /// success, calls `f` with exclusive access to the protected data.
    ///
    /// Unlike [`RwLock::try_with_write`], this function is allowed to
    /// spuriously fail even when acquiring exclusive write access would
    /// otherwise succeed, which can result in more efficient code on some
    /// platforms.
    #[inline]
    pub fn try_with_write_lock_weak<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        self.try_with_write_internal(false, f)
    }

    /// Obtains an upgradeable handle, blocking the current thread until it can
    /// be acquired, and calls `f` with it.
    ///
    /// Taking the upgradeable handle prevents new readers from acquiring the
    /// lock, but existing readers may still be around; the handle can be
    /// temporarily upgraded to exclusive access via [`Upgradeable::upgrade`].
    ///
    /// The lock is released once `f` returns, including when `f` unwinds.
    #[inline]
    pub fn with_upgradeable_read_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(Upgradeable<'_, T>) -> R,
    {
        let mut boff = Backoff::new();
        while !self.raw_try_upgradeable_read() {
            boff.spin();
        }

        f(Upgradeable { lock: self })
    }

    /// Attempts to obtain an upgradeable handle and, on success, calls `f` with
    /// it.
    ///
    /// Returns `None` without running `f` if a writer or another upgradeable
    /// handle currently holds the lock.
    #[inline]
    pub fn try_with_upgradeable_read_lock<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(Upgradeable<'_, T>) -> R,
    {
        if !self.raw_try_upgradeable_read() {
            return None;
        }

        Some(f(Upgradeable { lock: self }))
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut lock = spin::RwLock::new(0);
    /// *lock.get_mut() = 10;
    /// assert_eq!(lock.with_read_lock(|v| *v), 10);
    /// ```
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        // Safety: exclusive borrow of self means no critical section can be running.
        self.data.with_mut(|ptr| unsafe { &mut *ptr })
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

    #[inline(always)]
    fn try_with_write_internal<F, R>(&self, strong: bool, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        if !self.raw_try_write(strong) {
            return None;
        }
        let _release = ReleaseWrite(self);

        // Safety: the WRITER bit is held, so this thread has exclusive access.
        Some(self.data.with_mut(|ptr| f(unsafe { &mut *ptr })))
    }

    /// Bumps the reader count, returning the previous lock value.
    ///
    /// # Panics
    ///
    /// Panics if the reader count would overflow. The cap is arbitrary and sits
    /// far enough below `usize::MAX` that the overflow is caught long before it
    /// could wrap into the `WRITER`/`UPGRADED` bits.
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

    /// Attempts to acquire shared read access once, returning whether it was
    /// acquired.
    #[inline]
    fn raw_try_read(&self) -> bool {
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

    /// Releases shared read access.
    ///
    /// # Safety
    ///
    /// The caller must hold a read reference acquired via [`Self::raw_try_read`]
    /// (or handed over by [`Upgradeable::downgrade`]) and must not release it by
    /// other means.
    #[inline]
    unsafe fn raw_release_read(&self) {
        debug_assert!(self.lock.load(Ordering::Relaxed) & !(WRITER | UPGRADED) > 0);
        self.lock.fetch_sub(READER, Ordering::Release);
    }

    /// Attempts to acquire exclusive write access once, returning whether it
    /// was acquired.
    ///
    /// `strong` selects a non-spurious compare-exchange; pass `false` in a
    /// retry loop where a spurious failure costs nothing.
    #[inline(always)]
    fn raw_try_write(&self, strong: bool) -> bool {
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

    /// Releases exclusive write access.
    ///
    /// # Safety
    ///
    /// The caller must hold the `WRITER` bit and must not release it by other
    /// means.
    #[inline]
    unsafe fn raw_release_write(&self) {
        debug_assert_eq!(self.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        // Writer is responsible for clearing both WRITER and UPGRADED bits.
        // The UPGRADED bit may be set if an upgradeable lock attempts an upgrade while this lock is held.
        self.lock.fetch_and(!(WRITER | UPGRADED), Ordering::Release);
    }

    /// Attempts to acquire the `UPGRADED` bit once, returning whether it was
    /// acquired.
    #[inline]
    fn raw_try_upgradeable_read(&self) -> bool {
        // We can't unflip the UPGRADED bit back on failure as there is another
        // upgradeable or write lock. When they unlock, they will clear the bit.
        self.lock.fetch_or(UPGRADED, Ordering::Acquire) & (WRITER | UPGRADED) == 0
    }

    /// Releases the `UPGRADED` bit.
    ///
    /// # Safety
    ///
    /// The caller must hold the `UPGRADED` bit and must not release it by other
    /// means.
    #[inline]
    unsafe fn raw_release_upgradeable_read(&self) {
        debug_assert_eq!(
            self.lock.load(Ordering::Relaxed) & (WRITER | UPGRADED),
            UPGRADED
        );
        self.lock.fetch_sub(UPGRADED, Ordering::AcqRel);
    }

    /// Attempts to trade the `UPGRADED` bit for the `WRITER` bit once,
    /// returning whether the trade succeeded.
    ///
    /// The compare-exchange requires the lock word to be exactly `UPGRADED`, so
    /// this only succeeds once every remaining reader has left.
    ///
    /// # Safety
    ///
    /// The caller must hold the `UPGRADED` bit.
    #[inline(always)]
    unsafe fn raw_upgrade(&self, strong: bool) -> bool {
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

    /// Trades the `WRITER` bit back for the `UPGRADED` bit, without ever
    /// releasing the lock.
    ///
    /// Setting `UPGRADED` before clearing `WRITER` keeps the lock continuously
    /// blocked against readers, writers and other upgraders. Both steps are
    /// read-modify-write ops rather than a plain `store`, because a racing
    /// `raw_try_read` may have transiently bumped the reader count: a `store`
    /// would clobber that increment and its matching decrement would then
    /// underflow the lock word.
    ///
    /// # Safety
    ///
    /// The caller must hold the `WRITER` bit and must have acquired it via
    /// [`Self::raw_upgrade`], so that the `UPGRADED` bit is this thread's to own
    /// afterwards.
    #[inline]
    unsafe fn raw_downgrade_write_to_upgradeable(&self) {
        debug_assert_eq!(self.lock.load(Ordering::Relaxed) & WRITER, WRITER);

        self.lock.fetch_or(UPGRADED, Ordering::Relaxed);
        self.lock.fetch_and(!WRITER, Ordering::Release);
    }
}

/// Releases shared read access when dropped, so an unwind out of a closure
/// cannot leave the lock held forever.
struct ReleaseRead<'a, T: ?Sized>(&'a RwLock<T>);

impl<T: ?Sized> Drop for ReleaseRead<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: `ReleaseRead` is only constructed after this thread acquired
        // a read reference, and nothing else releases it in between.
        unsafe { self.0.raw_release_read() }
    }
}

/// Releases exclusive write access when dropped, so an unwind out of a closure
/// cannot leave the lock held forever.
struct ReleaseWrite<'a, T: ?Sized>(&'a RwLock<T>);

impl<T: ?Sized> Drop for ReleaseWrite<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: `ReleaseWrite` is only constructed after this thread acquired
        // the WRITER bit, and nothing else releases it in between.
        unsafe { self.0.raw_release_write() }
    }
}

impl<'a, T: ?Sized> Upgradeable<'a, T> {
    /// Returns a shared reference to the protected data.
    #[inline]
    pub fn get(&self) -> &T {
        // Safety: holding the UPGRADED bit grants shared read access.
        self.lock.data.with(|ptr| unsafe { &*ptr })
    }

    /// Upgrades to exclusive access for the duration of `f`, spinning until
    /// every remaining reader has left, then returns to upgradeable access.
    ///
    /// Takes `&mut self` so the borrow checker rules out handing out `&mut T`
    /// while a shared reference from [`Self::get`] is still alive.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    ///
    /// mylock.with_upgradeable_read_lock(|mut u| {
    ///     assert_eq!(*u.get(), 0);
    ///     u.upgrade(|w| *w += 1);
    ///     assert_eq!(*u.get(), 1);
    /// });
    /// ```
    #[inline]
    pub fn upgrade<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let mut boff = Backoff::new();
        // Safety: this handle owns the UPGRADED bit.
        while !unsafe { self.lock.raw_upgrade(false) } {
            boff.spin();
        }

        self.upgraded(f)
    }

    /// Attempts to upgrade to exclusive access for the duration of `f`,
    /// returning to upgradeable access afterwards.
    ///
    /// Returns `None` without running `f` if any other readers are currently
    /// holding the lock.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(0);
    ///
    /// mylock.with_upgradeable_read_lock(|mut u| {
    ///     match u.try_upgrade(|w| *w += 1) {
    ///         Some(()) => (), // upgrade successful
    ///         None => (),     // readers were still around
    ///     }
    /// });
    /// ```
    #[inline]
    pub fn try_upgrade<F, R>(&mut self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        // Safety: this handle owns the UPGRADED bit.
        if !unsafe { self.lock.raw_upgrade(true) } {
            return None;
        }

        Some(self.upgraded(f))
    }

    /// Attempts to upgrade to exclusive access for the duration of `f`,
    /// returning to upgradeable access afterwards.
    ///
    /// Unlike [`Upgradeable::try_upgrade`], this function is allowed to
    /// spuriously fail even when upgrading would otherwise succeed, which can
    /// result in more efficient code on some platforms.
    #[inline]
    pub fn try_upgrade_weak<F, R>(&mut self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R,
    {
        // Safety: this handle owns the UPGRADED bit.
        if !unsafe { self.lock.raw_upgrade(false) } {
            return None;
        }

        Some(self.upgraded(f))
    }

    /// Hands the lock over to shared read access for the duration of `f`.
    ///
    /// Consumes the handle, because the transfer is one-way: reclaiming the
    /// upgradeable state afterwards could spin, whereas this handoff is atomic
    /// and cannot — the read count is bumped before the `UPGRADED` bit is
    /// released, so no writer can slip in between.
    ///
    /// ```
    /// let mylock = spin::RwLock::new(1);
    ///
    /// mylock.with_upgradeable_read_lock(|u| {
    ///     assert!(mylock.try_with_read_lock(|_| ()).is_none());
    ///
    ///     u.downgrade(|r| {
    ///         // now a plain reader, so other readers may join
    ///         assert!(mylock.try_with_read_lock(|_| ()).is_some());
    ///         assert_eq!(*r, 1);
    ///     });
    /// });
    /// ```
    #[inline]
    pub fn downgrade<F, R>(self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        // Reserve the read reference for ourselves before giving up UPGRADED,
        // so no writer can acquire the lock in between.
        self.lock.acquire_reader();

        // `&RwLock<T>` is `Copy`, so this outlives dropping the handle below.
        let lock = self.lock;

        // Dropping the handle removes the UPGRADED bit.
        drop(self);

        let _release = ReleaseRead(lock);

        // Safety: this thread holds a read reference, so no writer can be active.
        lock.data.with(|ptr| f(unsafe { &*ptr }))
    }

    /// Runs `f` with exclusive access, then trades the `WRITER` bit back for the
    /// `UPGRADED` bit.
    ///
    /// The caller must have just won the `UPGRADED` -> `WRITER` trade.
    #[inline]
    fn upgraded<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        /// Restores the upgradeable state on every exit path, including unwind.
        struct Restore<'a, T: ?Sized>(&'a RwLock<T>);
        impl<T: ?Sized> Drop for Restore<'_, T> {
            #[inline]
            fn drop(&mut self) {
                // Safety: `Restore` is only constructed after a successful
                // `raw_upgrade`, so the WRITER bit is this thread's.
                unsafe { self.0.raw_downgrade_write_to_upgradeable() }
            }
        }
        let _restore = Restore(self.lock);

        // Safety: the WRITER bit is held, so this thread has exclusive access.
        self.lock.data.with_mut(|ptr| f(unsafe { &mut *ptr }))
    }
}

impl<T: ?Sized> Deref for Upgradeable<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.get()
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Upgradeable<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self.get(), f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for Upgradeable<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self.get(), f)
    }
}

impl<T: ?Sized> Drop for Upgradeable<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: this handle owns the UPGRADED bit for its whole lifetime.
        unsafe { self.lock.raw_release_upgradeable_read() }
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Bind the result before matching: the closure borrows `f` mutably, and
        // a match scrutinee's temporaries would keep that borrow alive across
        // the arms.
        let locked = self.try_with_read_lock(|data| {
            write!(f, "RwLock {{ data: ")
                .and_then(|()| data.fmt(f))
                .and_then(|()| write!(f, " }}"))
        });

        match locked {
            Some(res) => res,
            None => write!(f, "RwLock {{ <locked> }}"),
        }
    }
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T> From<T> for RwLock<T> {
    fn from(data: T) -> Self {
        Self::new(data)
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
            let l = RwLock::new(());
            l.with_read_lock(|_| ());
            l.with_write_lock(|_| ());
            l.with_read_lock(|_| l.with_read_lock(|_| ()));
            l.with_write_lock(|_| ());
        });
    }

    #[test]
    fn test_rwlock_unsized() {
        loom::model(|| {
            let rw: &RwLock<[i32]> = &RwLock::new([1, 2, 3]);
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
            let lock = RwLock::new(0isize);
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
            let m = RwLock::new(0);
            m.with_write_lock(|_| {
                assert!(m.try_with_read_lock(|_| ()).is_none());
            });
        })
    }

    #[test]
    fn test_readers_block_writers() {
        loom::model(|| {
            let m = RwLock::new(());

            m.with_read_lock(|_| {
                m.with_read_lock(|_| {
                    m.with_read_lock(|_| {
                        assert!(m.try_with_write_lock(|_| ()).is_none());
                    });
                    assert!(m.try_with_write_lock(|_| ()).is_none());
                });
                assert!(m.try_with_write_lock(|_| ()).is_none());
            });

            // Every read reference has been released, so a writer gets in.
            assert!(m.try_with_write_lock(|_| ()).is_some());
        })
    }

    #[test]
    fn test_released_on_unwind() {
        loom::model(|| {
            let m = RwLock::new(0);

            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                m.with_write_lock(|v| {
                    *v = 7;
                    panic!("boom");
                })
            }));

            assert!(res.is_err());
            assert!(!m.is_locked(), "the lock must be released on unwind");
            assert_eq!(m.try_with_read_lock(|v| *v), Some(7));
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
    fn test_upgrade_downgrade() {
        loom::model(|| {
            let m = RwLock::new(());

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

            // With no readers around, the upgrade succeeds and the handle is
            // usable again afterwards.
            m.with_upgradeable_read_lock(|mut u| {
                assert!(u.try_upgrade(|_| ()).is_some());
                assert!(m.try_with_read_lock(|_| ()).is_none());
                assert!(m.try_with_write_lock(|_| ()).is_none());
            });

            assert!(m.try_with_write_lock(|_| ()).is_some());
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
                static ref L: RwLock<usize> = RwLock::new(0);
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
                static ref L: RwLock<()> = RwLock::new(());
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
                static ref L: RwLock<usize> = RwLock::new(0);
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
                static ref L: RwLock<usize> = RwLock::new(0);
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
                static ref L: RwLock<()> = RwLock::new(());
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
                static ref L: RwLock<()> = RwLock::new(());
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
