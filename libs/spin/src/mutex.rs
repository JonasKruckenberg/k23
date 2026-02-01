// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::hint;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::Backoff;

pub type Mutex<T> = lock_api::Mutex<RawMutex, T>;
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutex, T>;
pub type MappedMutexGuard<'a, T> = lock_api::MappedMutexGuard<'a, RawMutex, T>;

pub struct RawMutex {
    lock: AtomicBool,
}

impl RawMutex {
    #[inline(always)]
    fn try_lock_internal(&self, strong: bool) -> bool {
        if strong {
            self.lock
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        } else {
            self.lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        }
    }
}

#[allow(clippy::undocumented_unsafe_blocks, reason = "TODO")]
unsafe impl lock_api::RawMutex for RawMutex {
    type GuardMarker = lock_api::GuardSend;

    const INIT: Self = Self {
        lock: AtomicBool::new(false),
    };

    fn lock(&self) {
        let mut boff = Backoff::new();

        while !self.try_lock_internal(false) {
            hint::cold_path();
            while self.is_locked() {
                boff.spin();
            }
        }
    }

    fn try_lock(&self) -> bool {
        self.try_lock_internal(true)
    }

    unsafe fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::undocumented_unsafe_blocks, reason = "is fine in tests")]

    use core::sync::atomic::AtomicUsize;

    use lock_api::RawMutex as _;

    use super::*;
    use crate::loom;
    use crate::loom::thread;

    /// Number of cycles to repeat concurrency tests for, but loom and miri are really slow
    /// and should probably catch any bugs after much fewer iterations anyway
    const CYCLES: usize = if cfg!(loom) | cfg!(miri) { 100 } else { 500 };

    #[test]
    fn correctness() {
        /// Size of the mutex-protected data, miri is reeeally slow for any substantial buffer size,
        /// but should catch any potential issues with smaller buffers anyway
        const BUF_SIZE: usize = if cfg!(miri) { 8 } else { 1024 };

        loom::model(|| {
            loom::lazy_static! {
                static ref M: Mutex<[u8; BUF_SIZE]> = Mutex::new([0u8; BUF_SIZE]);
            }

            let mut threads = Vec::new();
            for _ in 0..loom::MAX_THREADS - 1 {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let mut guard = M.lock();

                        assert!(guard.iter().all(|b| *b == guard[0]));

                        guard.fill(rand::random());

                        drop(guard);
                        #[cfg(loom)]
                        thread::yield_now();
                    }
                }));
            }

            for t in threads {
                t.join().unwrap();
            }
        });
    }

    #[test]
    fn stress_test() {
        loom::model(|| {
            loom::lazy_static! {
                static ref M: RawMutex = RawMutex::INIT;
                static ref DATA: AtomicUsize = AtomicUsize::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..loom::MAX_THREADS - 1 {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        M.lock();
                        assert_eq!(DATA.fetch_add(1, Ordering::Relaxed), 0);
                        assert_eq!(DATA.fetch_sub(1, Ordering::Relaxed), 1);
                        unsafe { M.unlock() };

                        #[cfg(loom)]
                        thread::yield_now();
                    }
                }));
            }

            for t in threads {
                t.join().unwrap();
            }
        });
    }

    #[test]
    fn stress_test_try_lock() {
        loom::model(|| {
            loom::lazy_static! {
                static ref M: RawMutex = RawMutex::INIT;
                static ref DATA: AtomicUsize = AtomicUsize::new(0);
            }

            let mut threads = Vec::new();
            for _ in 0..loom::MAX_THREADS - 1 {
                threads.push(thread::spawn(|| {
                    for _ in 0..CYCLES {
                        let mut boff = Backoff::new();
                        while !M.try_lock() {
                            boff.spin();
                        }

                        assert_eq!(DATA.fetch_add(1, Ordering::Relaxed), 0);
                        assert_eq!(DATA.fetch_sub(1, Ordering::Relaxed), 1);
                        unsafe { M.unlock() };

                        #[cfg(loom)]
                        thread::yield_now();
                    }
                }));
            }

            for t in threads {
                t.join().unwrap();
            }
        });
    }
}
