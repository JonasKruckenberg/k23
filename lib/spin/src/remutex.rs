// Copyright 2025. Jonas Kruckenberg
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

use util::loom_const_fn;

use crate::Backoff;
use crate::loom::cell::UnsafeCell;
use crate::loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Marker type indicating a guard is not `Send`.
#[expect(dead_code, reason = "inner pointer is unused")]
struct GuardNoSend(*mut ());

// Safety: only sync, never send.
unsafe impl Sync for GuardNoSend {}

/// A mutex which can be recursively locked by a single thread.
///
/// This type is identical to [`crate::Mutex`] except that:
///
/// - Locking multiple times from the same thread works instead of deadlocking.
/// - [`ReentrantMutexGuard`] only gives shared references to the locked data;
///   use a `RefCell` inside if you need interior mutability.
pub struct ReentrantMutex<T: ?Sized> {
    owner: AtomicUsize,
    lock_count: Cell<usize>,
    lock: AtomicBool,
    data: UnsafeCell<T>,
}

/// An RAII implementation of a "scoped lock" of a reentrant mutex. When this
/// structure is dropped (falls out of scope), the lock will be unlocked.
#[clippy::has_significant_drop]
#[must_use = "if unused the ReentrantMutex will immediately unlock"]
pub struct ReentrantMutexGuard<'a, T: ?Sized> {
    remutex: &'a ReentrantMutex<T>,
    marker: PhantomData<(&'a T, GuardNoSend)>,
}

// Safety: synchronization is provided internally.
unsafe impl<T: ?Sized + Send> Send for ReentrantMutex<T> {}
// Safety: synchronization is provided internally.
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

impl<T: ?Sized> ReentrantMutex<T> {
    /// Acquires the reentrant mutex, spinning until available.
    #[inline]
    pub fn lock(&self) -> ReentrantMutexGuard<'_, T> {
        let locked = self.lock_internal(|| {
            let mut boff = Backoff::new();
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

        // Safety: we have just ensured the mutex is locked by this thread.
        unsafe { self.make_guard_unchecked() }
    }

    /// Attempts to acquire the reentrant mutex without spinning.
    #[inline]
    pub fn try_lock(&self) -> Option<ReentrantMutexGuard<'_, T>> {
        let locked = self.lock_internal(|| {
            self.lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        });

        if locked {
            // Safety: we have just ensured the mutex is locked by this thread.
            Some(unsafe { self.make_guard_unchecked() })
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying data.
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

    /// Returns `true` if the mutex is held by the current thread.
    #[inline]
    pub fn is_owned_by_current_thread(&self) -> bool {
        let id = nonzero_thread_id().get();
        self.owner.load(Ordering::Relaxed) == id
    }

    #[inline]
    unsafe fn make_guard_unchecked(&self) -> ReentrantMutexGuard<'_, T> {
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

    /// # Safety
    ///
    /// Must only be called when the current thread logically owns a
    /// `ReentrantMutexGuard` but has discarded it via `mem::forget`.
    #[inline]
    unsafe fn force_unlock(&self) {
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
        ReentrantMutex::new(T::default())
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
            None => f
                .debug_struct("ReentrantMutex")
                .field("data", &format_args!("<locked>"))
                .finish(),
        }
    }
}

// Safety: the guard gives only shared access, so sharing across threads is OK.
unsafe impl<T: ?Sized + Sync> Sync for ReentrantMutexGuard<'_, T> {}

impl<'a, T: ?Sized + 'a> Deref for ReentrantMutexGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        // Safety: the guard holds the lock.
        self.remutex.data.with(|data| unsafe { &*data })
    }
}

impl<T: ?Sized> Drop for ReentrantMutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // Safety: the guard holds the lock.
        unsafe {
            self.remutex.force_unlock();
        }
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for ReentrantMutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for ReentrantMutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

fn nonzero_thread_id() -> NonZeroUsize {
    #[thread_local]
    static X: u8 = 0;
    NonZeroUsize::new(addr_of!(X) as usize).expect("thread ID was zero")
}
