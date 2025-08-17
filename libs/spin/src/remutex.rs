// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cell::Cell;
use core::fmt;
use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::ops::Deref;
use core::ptr::addr_of;
use core::sync::atomic::AtomicBool;

use util::loom_const_fn;

use crate::loom::cell::UnsafeCell;
use crate::loom::sync::atomic::{AtomicUsize, Ordering};
use crate::{Backoff, GuardNoSend};

/// A mutex which can be recursively locked by a single thread.
///
/// This type is identical to `Mutex` except for the following points:
///
/// - Locking multiple times from the same thread will work correctly instead of
///   deadlocking.
/// - `ReentrantMutexGuard` does not give mutable references to the locked data.
///   Use a `RefCell` if you need this.
///
/// See [`Mutex`](crate::Mutex) for more details about the underlying mutex
/// primitive.
pub struct ReentrantMutex<T: ?Sized> {
    owner: AtomicUsize,
    lock_count: Cell<usize>,
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

/// An RAII implementation of a "scoped lock" of a reentrant mutex. When this structure
/// is dropped (falls out of scope), the lock will be unlocked.
///
/// The data protected by the mutex can be accessed through this guard via its
/// `Deref` implementation.
#[clippy::has_significant_drop]
#[must_use = "if unused the ReentrantMutex will immediately unlock"]
pub struct ReentrantMutexGuard<'a, T: ?Sized> {
    remutex: &'a ReentrantMutex<T>,
    marker: PhantomData<(&'a T, GuardNoSend)>,
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<T: ?Sized + Send> Send for ReentrantMutex<T> {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<T: ?Sized + Send> Sync for ReentrantMutex<T> {}

impl<T> ReentrantMutex<T> {
    loom_const_fn! {
        /// Creates a new reentrant mutex in an unlocked state ready for use.
        #[inline]
        pub const fn new(val: T) -> ReentrantMutex<T> {
            ReentrantMutex {
                owner: AtomicUsize::new(0),
                lock_count: Cell::new(0),
                lock: AtomicBool::new(false),
                data: UnsafeCell::new(val)
            }
        }
    }

    /// Consumes this mutex, returning the underlying data.
    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> ReentrantMutex<T> {
    /// Creates a new `ReentrantMutexGuard` without checking if the lock is held.
    ///
    /// # Safety
    ///
    /// This method must only be called if the thread logically holds the lock.
    ///
    /// Calling this function when a guard has already been produced is undefined behaviour unless
    /// the guard was forgotten with `mem::forget`.
    #[inline]
    pub unsafe fn make_guard_unchecked(&self) -> ReentrantMutexGuard<'_, T> {
        ReentrantMutexGuard {
            remutex: self,
            marker: PhantomData,
        }
    }

    #[inline]
    fn lock_internal<F: FnOnce() -> bool>(&self, lock_inner: F) -> bool {
        let id = nonzero_thread_id().get();

        if self.owner.load(Ordering::Relaxed) == id {
            self.lock_count.set(
                self.lock_count
                    .get()
                    .checked_add(1)
                    .expect("ReentrantMutex lock count overflow"),
            );
        } else {
            if !lock_inner() {
                return false;
            }
            self.owner.store(id, Ordering::Relaxed);
            debug_assert_eq!(self.lock_count.get(), 0);
            self.lock_count.set(1);
        }
        true
    }

    /// Acquires a reentrant mutex, blocking the current thread until it is able
    /// to do so.
    ///
    /// If the mutex is held by another thread then this function will block the
    /// local thread until it is available to acquire the mutex. If the mutex is
    /// already held by the current thread then this function will increment the
    /// lock reference count and return immediately. Upon returning,
    /// the thread is the only thread with the mutex held. An RAII guard is
    /// returned to allow scoped unlock of the lock. When the guard goes out of
    /// scope, the mutex will be unlocked.
    #[inline]
    pub fn lock(&self) -> ReentrantMutexGuard<'_, T> {
        let locked = self.lock_internal(|| {
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

            true
        });
        debug_assert!(locked);

        // Safety: we have just ensured the mutex is locked by this thread
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
    pub fn try_lock(&self) -> Option<ReentrantMutexGuard<'_, T>> {
        let locked = self.lock_internal(|| {
            self.lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        });

        if locked {
            // Safety: we have just ensured the mutex is locked by this thread
            unsafe { Some(self.make_guard_unchecked()) }
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying data.
    ///
    /// Since this call borrows the `ReentrantMutex` mutably, no actual locking needs to
    /// take place---the mutable borrow statically guarantees no locks exist.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.with_mut(|data| {
            // Safety: We hold a mutable reference to the RwLock so getting a mutable reference to the
            // data is safe
            unsafe { &mut *data }
        })
    }

    /// Checks whether the mutex is currently locked.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    /// Checks whether the mutex is currently held by the current thread.
    #[inline]
    pub fn is_owned_by_current_thread(&self) -> bool {
        let id = nonzero_thread_id().get();
        self.owner.load(Ordering::Relaxed) == id
    }

    /// Forcibly unlocks the mutex.
    ///
    /// This is useful when combined with `mem::forget` to hold a lock without
    /// the need to maintain a `ReentrantMutexGuard` object alive, for example when
    /// dealing with FFI.
    ///
    /// # Safety
    ///
    /// This method must only be called if the current thread logically owns a
    /// `ReentrantMutexGuard` but that guard has be discarded using `mem::forget`.
    /// Behavior is undefined if a mutex is unlocked when not locked.
    #[inline]
    pub unsafe fn force_unlock(&self) {
        let lock_count = self.lock_count.get() - 1;
        self.lock_count.set(lock_count);
        if lock_count == 0 {
            self.owner.store(0, Ordering::Relaxed);
            self.lock.store(false, Ordering::Release);
        }
    }
}

impl<T: Default> Default for ReentrantMutex<T> {
    #[inline]
    fn default() -> ReentrantMutex<T> {
        ReentrantMutex::new(Default::default())
    }
}

impl<T> From<T> for ReentrantMutex<T> {
    #[inline]
    fn from(t: T) -> ReentrantMutex<T> {
        ReentrantMutex::new(t)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for ReentrantMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => f
                .debug_struct("ReentrantMutex")
                .field("data", &&*guard)
                .finish(),
            None => {
                struct LockedPlaceholder;
                impl fmt::Debug for LockedPlaceholder {
                    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("<locked>")
                    }
                }

                f.debug_struct("ReentrantMutex")
                    .field("data", &LockedPlaceholder)
                    .finish()
            }
        }
    }
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl<'a, T: ?Sized + Sync + 'a> Sync for ReentrantMutexGuard<'a, T> {}

impl<'a, T: ?Sized + Sync + 'a> ReentrantMutexGuard<'a, T> {
    /// Returns a reference to the original `ReentrantMutex` object.
    pub fn remutex(s: &Self) -> &'a ReentrantMutex<T> {
        s.remutex
    }
}

impl<'a, T: ?Sized + 'a> Deref for ReentrantMutexGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.remutex.data.with(|data| {
            // Safety: ReentrantMutexGuard always holds the lock, so it is safe to access the data
            unsafe { &*data }
        })
    }
}

impl<'a, T: ?Sized + 'a> Drop for ReentrantMutexGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: A ReentrantMutexGuard always holds the lock.
        unsafe {
            self.remutex.force_unlock();
        }
    }
}

impl<'a, T: fmt::Debug + ?Sized + 'a> fmt::Debug for ReentrantMutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<'a, T: fmt::Display + ?Sized + 'a> fmt::Display for ReentrantMutexGuard<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

fn nonzero_thread_id() -> NonZeroUsize {
    #[thread_local]
    static X: u8 = 0;
    NonZeroUsize::new(addr_of!(X) as usize).expect("thread ID was zero")
}
