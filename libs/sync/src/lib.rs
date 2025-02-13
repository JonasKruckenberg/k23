// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Synchronization primitives for use in k23.
#![no_std]
#![cfg_attr(feature = "thread-local", feature(thread_local))]

mod backoff;
mod barrier;
mod lazy_lock;
mod once;
mod once_lock;
mod raw_mutex;
mod raw_rwlock;
#[cfg(feature = "thread-local")]
mod reentrant_mutex;

pub use raw_mutex::RawMutex;
pub use raw_rwlock::RawRwLock;

pub use backoff::Backoff;
pub use barrier::{Barrier, BarrierWaitResult};
pub use lazy_lock::LazyLock;
pub use once::Once;
pub use once_lock::OnceLock;

/// A mutual exclusion lock.
pub type Mutex<T> = lock_api::Mutex<RawMutex, T>;
/// RAII structure used to release the lock when dropped.
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutex, T>;
/// RAII structure used to release lock when dropped, which can point to a subfield of the protected data.
pub type MappedMutexGuard<'a, T> = lock_api::MappedMutexGuard<'a, RawMutex, T>;

/// A reader-writer lock.
pub type RwLock<T> = lock_api::RwLock<RawRwLock, T>;
/// RAII structure used to release the exclusive write access of a lock when dropped.
pub type RwLockReadGuard<'a, T> = lock_api::RwLockReadGuard<'a, RawRwLock, T>;
/// RAII structure used to release the shared read access of a lock when dropped.
pub type RwLockWriteGuard<'a, T> = lock_api::RwLockWriteGuard<'a, RawRwLock, T>;
/// `RwLockReadGuard` which can be upgraded to a `RwLockWriteGuard`.
pub type RwLockUpgradableReadGuard<'a, T> = lock_api::RwLockUpgradableReadGuard<'a, RawRwLock, T>;
/// RAII structure used to release the shared read access of a lock when dropped, which can point to a subfield of the protected data.
pub type MappedRwLockReadGuard<'a, T> = lock_api::MappedRwLockReadGuard<'a, RawRwLock, T>;
/// RAII structure used to release the exclusive write access of a lock when dropped, which can point to a subfield of the protected data.
pub type MappedRwLockWriteGuard<'a, T> = lock_api::MappedRwLockWriteGuard<'a, RawRwLock, T>;

#[cfg(feature = "thread-local")]
pub use reentrant_mutex::*;
