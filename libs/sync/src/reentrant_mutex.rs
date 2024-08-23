//! Sync primitives that require thread-local storage.

use crate::RawMutex;
use core::num::NonZeroUsize;
use core::ptr::addr_of;
use lock_api::GetThreadId;

/// A mutex which can be recursively locked by a single thread.
///
/// There are two relevant differences between `ReentrantMutex` and `Mutex`:
/// 1. `ReentrantMutex` can be locked multiple times by the same thread without deadlocking.
/// 2. A `ReentrantMutexGuard` does not give mutable references to the locked data. A [`RefCell`](core::cell::RefCell) can be used to achieve this.
///
///
pub type ReentrantMutex<T> = lock_api::ReentrantMutex<RawMutex, LocalThreadId, T>;
/// RAII structure used to release reentrant lock when dropped.
///
/// If the lock has been held recursively the lock will be released when the last `ReentrantMutexGuard` is dropped.
///
/// # Mutability
///
/// Unlike [`MutexGuard`](crate::MutexGuard) this guard does not implement [`DerefMut`](core::ops::DerefMut)
/// since that could be used to obtain multiple mutable references to the locked data and that
/// would violate Rust's reference aliasing rules.
pub type ReentrantMutexGuard<'a, T> = lock_api::ReentrantMutexGuard<'a, RawMutex, LocalThreadId, T>;
/// RAII structure used to release reentrant lock when dropped, which can point to a subfield of the protected data.
pub type MappedReentrantMutexGuard<'a, T> =
    lock_api::MappedReentrantMutexGuard<'a, RawMutex, LocalThreadId, T>;

/// A unique identifier for the calling thread.
///
/// This is an opaque object that uniquely identifies the calling thread. Importantly, the underlying
/// value is *not* human-readable, sequential or even stable across versions or runs.
pub struct LocalThreadId;

unsafe impl GetThreadId for LocalThreadId {
    const INIT: Self = LocalThreadId;

    fn nonzero_thread_id(&self) -> NonZeroUsize {
        // The address of a thread-local variable is guaranteed to be unique t<o the
        // current thread, and is also guaranteed to be non-zero. The variable has to have a
        // non-zero size to guarantee it has a unique address for each thread.>
        #[thread_local]
        static X: u8 = 0;
        NonZeroUsize::new(addr_of!(X) as usize).expect("thread ID was zero")
    }
}
