// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Synchronization primitives for use in k23.

#![feature(cold_path)]
#![cfg_attr(not(test), no_std)]
#![cfg_attr(feature = "thread-local", feature(thread_local))]
#![feature(dropck_eyepatch)]

mod backoff;
mod barrier;
mod lazy_lock;
mod loom;
mod mutex;
mod once;
mod once_lock;
#[cfg(feature = "thread-local")]
mod remutex;
mod rw_lock;

pub use backoff::Backoff;
pub use barrier::{Barrier, BarrierWaitResult};
pub use lazy_lock::LazyLock;
pub use mutex::{MappedMutexGuard, Mutex, MutexGuard, RawMutex};
pub use once::{ExclusiveState, Once};
pub use once_lock::OnceLock;
#[cfg(feature = "thread-local")]
pub use remutex::{GetCpuId, MappedReentrantMutexGuard, ReentrantMutex, ReentrantMutexGuard};
pub use rw_lock::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RawRwLock, RwLock, RwLockReadGuard,
    RwLockUpgradableReadGuard, RwLockWriteGuard,
};
