// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::num::NonZeroUsize;
use core::ptr::addr_of;

use crate::RawMutex;

pub type ReentrantMutex<T> = lock_api::ReentrantMutex<RawMutex, GetCpuId, T>;
pub type ReentrantMutexGuard<'a, T> = lock_api::ReentrantMutexGuard<'a, RawMutex, GetCpuId, T>;
pub type MappedReentrantMutexGuard<'a, T> =
    lock_api::MappedReentrantMutexGuard<'a, RawMutex, GetCpuId, T>;

pub struct GetCpuId(());

// Safety: TODO
unsafe impl lock_api::GetThreadId for GetCpuId {
    const INIT: Self = Self(());

    fn nonzero_thread_id(&self) -> NonZeroUsize {
        #[thread_local]
        static X: u8 = 0;
        NonZeroUsize::new(addr_of!(X) as usize).expect("thread ID was zero")
    }
}
