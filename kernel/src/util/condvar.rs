// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::time::Instant;
use crate::util::parking_spot::{ParkingSpot, WaitResult, Waiter};
use core::ops::Add;
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};
use core::time::Duration;
use lock_api::RawMutex as _;
use sync::{MutexGuard, RawMutex};

pub struct Condvar {
    state: AtomicPtr<RawMutex>,
}

impl Condvar {
    #[inline]
    pub const fn new() -> Condvar {
        Condvar {
            state: AtomicPtr::new(ptr::null_mut()),
        }
    }

    #[inline]
    pub fn notify_one(&self, parking_spot: &ParkingSpot) -> bool {
        // Nothing to do if there are no waiting threads
        let state = self.state.load(Ordering::Relaxed);
        if state.is_null() {
            return false;
        }

        self.notify_one_slow(parking_spot, state)
    }

    #[cold]
    fn notify_one_slow(&self, parking_spot: &ParkingSpot, mutex: *mut RawMutex) -> bool {
        let unparked_threads = parking_spot.notify(mutex as u64, 1);

        unparked_threads == 1
    }

    #[inline]
    pub fn notify_all(&self, parking_spot: &ParkingSpot) -> u32 {
        // Nothing to do if there are no waiting threads
        let state = self.state.load(Ordering::Relaxed);
        if state.is_null() {
            return 0;
        }

        self.notify_all_slow(parking_spot, state)
    }

    #[cold]
    fn notify_all_slow(&self, parking_spot: &ParkingSpot, mutex: *mut RawMutex) -> u32 {
        parking_spot.notify(mutex as u64, u32::MAX)
    }

    #[inline]
    pub fn wait<T: ?Sized>(&self, parking_spot: &ParkingSpot, mutex_guard: &mut MutexGuard<'_, T>) {
        self.wait_until_internal(
            parking_spot,
            unsafe { MutexGuard::mutex(mutex_guard).raw() },
            None,
        );
    }

    #[inline]
    pub fn wait_until<T: ?Sized>(
        &self,
        parking_spot: &ParkingSpot,
        mutex_guard: &mut MutexGuard<'_, T>,
        deadline: Instant,
    ) -> WaitResult {
        self.wait_until_internal(
            parking_spot,
            unsafe { MutexGuard::mutex(mutex_guard).raw() },
            Some(deadline),
        )
    }

    #[inline]
    pub fn wait_for<T: ?Sized>(
        &self,
        parking_spot: &ParkingSpot,
        mutex_guard: &mut MutexGuard<'_, T>,
        duration: Duration,
    ) -> WaitResult {
        self.wait_until_internal(
            parking_spot,
            unsafe { MutexGuard::mutex(mutex_guard).raw() },
            Some(Instant::now().add(duration)),
        )
    }

    fn wait_until_internal(
        &self,
        parking_spot: &ParkingSpot,
        mutex: &RawMutex,
        deadline: Option<Instant>,
    ) -> WaitResult {
        let lock_addr = mutex as *const RawMutex as *mut RawMutex;

        let mut waiter = Waiter::new();
        let mut bad_mutex = false;

        let validate = || {
            // Ensure we don't use two different mutexes with the same
            // Condvar at the same time. This is done while locked to
            // avoid races with notify_one
            let state = self.state.load(Ordering::Relaxed);
            if state.is_null() {
                self.state.store(lock_addr, Ordering::Relaxed);
            } else if state != lock_addr {
                bad_mutex = true;
                return false;
            }
            true
        };

        // Unlock the mutex before sleeping...
        unsafe { mutex.unlock() };

        let res = parking_spot.wait(
            lock_addr as u64,
            validate,
            deadline,
            &mut waiter,
        );
        log::trace!("here {res:?}");

        // Panic if we tried to use multiple mutexes with a Condvar. Note
        // that at this point the MutexGuard is still locked. It will be
        // unlocked by the unwinding logic.
        if bad_mutex {
            panic!("attempted to use a condition variable with more than one mutex");
        }
        
        // Relock the mutex after sleeping...
        mutex.lock();

        res
    }
}
