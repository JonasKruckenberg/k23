// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Synchronization primitives for use in k23.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(feature = "thread-local", feature(thread_local))]
#![feature(dropck_eyepatch)]
#![feature(negative_impls)]

mod backoff;
mod barrier;
mod lazy_lock;
mod loom;
mod mutex;
mod once;
mod once_lock;
mod rw_lock;

#[cfg(feature = "thread-local")]
mod remutex;

pub use backoff::Backoff;
pub use barrier::{Barrier, BarrierWaitResult};
pub use lazy_lock::LazyLock;
pub use mutex::{Mutex, MutexGuard, RawMutex};
pub use once::{ExclusiveState, Once};
pub use once_lock::OnceLock;
#[cfg(feature = "thread-local")]
pub use remutex::{ReentrantMutex, ReentrantMutexGuard};
pub use rw_lock::{RwLock, RwLockReadGuard, RwLockUpgradableReadGuard, RwLockWriteGuard};

/// Marker type which indicates that the Guard type for a lock is not `Send`.
#[expect(dead_code, reason = "inner pointer is unused")]
pub(crate) struct GuardNoSend(*mut ());
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Sync for GuardNoSend {}
