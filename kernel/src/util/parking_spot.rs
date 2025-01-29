// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::time::Instant;
use crate::{arch};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::mem::offset_of;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use sync::Mutex;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WaitResult {
    /// Indicates that a `wait` completed by being awoken by a different thread.
    /// This means the thread went to sleep and didn't time out.
    Ok = 0,
    /// Indicates that `wait` did not complete and instead returned due to the
    /// value in memory not matching the expected value.
    Mismatch = 1,
    /// Indicates that `wait` completed with a timeout, meaning that the
    /// original value matched as expected but nothing ever called `notify`.
    TimedOut = 2,
}

/// The thread global `ParkingSpot`.
#[derive(Default, Debug)]
pub struct ParkingSpot {
    inner: Mutex<BTreeMap<u64, Spot>>,
}

#[derive(Default)]
pub struct Waiter {
    inner: Option<Box<WaiterInner>>,
}

#[derive(Default, Debug)]
struct Spot(linked_list::List<WaiterInner>);

struct WaiterInner {
    links: linked_list::Links<Self>,
    // NB: these fields are only modified/read under the lock of a
    // `ParkingSpot`.
    notified: bool,
    hartid: usize,
}

impl Waiter {
    pub const fn new() -> Waiter {
        Waiter { inner: None }
    }
}

impl ParkingSpot {
    /// Atomically validates if `atomic == expected` and, if so, blocks the
    /// current thread.
    ///
    /// This method will first check to see if `atomic == expected` using a
    /// `SeqCst` load ordering. If the values are not equal then the method
    /// immediately returns with `WaitResult::Mismatch`. Otherwise the thread
    /// will be blocked and can only be woken up with `notify` on the same
    /// address. Note that the check-and-block operation is atomic with respect
    /// to `notify`.
    ///
    /// The optional `deadline` specified can indicate a point in time after
    /// which this thread will be unblocked. If this thread is not notified and
    /// `deadline` is reached then `WaitResult::TimedOut` is returned. If
    /// `deadline` is `None` then this thread will block forever waiting for
    /// `notify`.
    ///
    /// The `waiter` argument is metadata used by this structure to block
    /// the current thread.
    ///
    /// This method will not spuriously wake up one blocked.
    pub fn wait32(
        &self,
        atomic: &AtomicU32,
        expected: u32,
        deadline: Option<Instant>,
        waiter: &mut Waiter,
    ) -> WaitResult {
        self.wait(
            atomic.as_ptr() as u64,
            || atomic.load(Ordering::SeqCst) == expected,
            deadline,
            waiter,
        )
    }

    /// Same as `wait32`, but for 64-bit values.
    pub fn wait64(
        &self,
        atomic: &AtomicU64,
        expected: u64,
        deadline: Option<Instant>,
        waiter: &mut Waiter,
    ) -> WaitResult {
        self.wait(
            atomic.as_ptr() as u64,
            || atomic.load(Ordering::SeqCst) == expected,
            deadline,
            waiter,
        )
    }

    pub fn wait(
        &self,
        key: u64,
        validate: impl FnOnce() -> bool,
        deadline: Option<Instant>,
        waiter: &mut Waiter,
    ) -> WaitResult {
        let mut inner = self.inner.lock();

        // This is the "atomic" part of the `validate` check which ensure that
        // the memory location still indicates that we're allowed to block.
        if !validate() {
            return WaitResult::Mismatch;
        }

        // Lazily initialize the `waiter` node if it hasn't been already, and
        // additionally ensure it's not accidentally in some other queue.
        let waiter = waiter.inner.get_or_insert_with(|| {
            Box::new(WaiterInner {
                links: linked_list::Links::default(),
                notified: false,
                hartid: crate::HARTID.get(),
            })
        });

        // Clear the `notified` flag if it was previously notified and
        // configure the thread to wakeup as our own.
        waiter.notified = false;
        waiter.hartid = crate::HARTID.get();

        let ptr = NonNull::from(&mut **waiter);
        let spot = inner.entry(key).or_default();

        // Safety: the section below is incredibly critical, calling `arch::hart_park` or `arch::hart_park_timeout`
        // will suspend the calling hart and potentially deadlock
        unsafe {
            // Enqueue our `waiter` in the internal queue for this spot.
            spot.0.push_back(ptr);

            // Wait for a notification to arrive. This is done through
            // `arch::hart_park_timeout` by dropping the lock that is held.
            // This loop is somewhat similar to a condition variable.
            //
            // If no timeout was given then the maximum duration is effectively
            // infinite (500 billion years), otherwise the timeout is
            // calculated relative to the `deadline` specified.
            //
            // To handle spurious wakeups if the thread wakes up but a
            // notification wasn't received then the thread goes back to sleep.
            let timed_out = loop {
                if let Some(deadline) = deadline {
                    let now = Instant::now();
                    let timeout = if deadline <= now {
                        break true;
                    } else {
                        deadline - now
                    };

                    // Suspend the calling hart for at least `timeout` duration.
                    // This will put the hart into a "wait for interrupt" mode where it will wait until
                    // either the timeout interrupt arrives or it receives an interrupt from another hart.
                    drop(inner);
                    log::trace!("parking for {timeout:?}...");
                    arch::hart_park_timeout(timeout);
                    log::trace!("unparked!");
                    inner = self.inner.lock();
                } else {
                    // Suspend the calling hart for an indefinite amount of time.
                    // It will only wake up if it receives an interrupt from another hart.
                    drop(inner);
                    log::trace!("parking indefinitely...");
                    arch::hart_park();
                    log::trace!("unparked!");
                    inner = self.inner.lock();
                }

                if ptr.as_ref().notified {
                    break false;
                }
            };

            if timed_out {
                // If this thread timed out then it is still present in the
                // waiter queue, so remove it.
                inner
                    .get_mut(&key)
                    .unwrap()
                    .0
                    .cursor_from_ptr_mut(ptr)
                    .remove();
                WaitResult::TimedOut
            } else {
                // If this node was notified then we should not be in a queue
                // at this point.
                assert!(!linked_list::Linked::links(ptr).as_ref().is_linked());
                WaitResult::Ok
            }
        }
    }

    pub fn notify(&self, addr: u64, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let mut unparked = 0;

        // It's known here that `n > 0` so dequeue items until `unparked`
        // equals `n` or the queue runs out. Each thread dequeued is signaled
        // that it's been notified and then woken up.
        self.with_lot(addr, |spot| {
            while let Some(mut head) = spot.0.pop_front() {
                // Safety: linked-list ensures that pointers are valid.
                let head = unsafe { head.as_mut() };
                head.notified = true;

                // Send an interrupt to the hart to wake it up, if the hart is already running (which
                // shouldn't happen) then this will do nothing, but if the hart is parked at the call
                // to `arch::hart_park_timeout` above then this will wake it up and it will return from
                // the call to `wait`.
                // Safety: This will send an interrupt to the target hart to wake it up, if the hart
                // is already running then the implementation of the trap handler ensures "nothing
                // happens" i.e. the trap handler just temporarily interrupts the running code, does
                // nothing and then return. But we have to be very careful here to ensure that the
                // interrupt doesn't blow up the target hart.
                unsafe {
                    arch::hart_unpark(head.hartid);
                }

                unparked += 1;
                if unparked == n {
                    break;
                }
            }
        });

        unparked
    }

    fn with_lot<F: FnMut(&mut Spot)>(&self, addr: u64, mut f: F) {
        let mut inner = self.inner.lock();
        if let Some(spot) = inner.get_mut(&addr) {
            f(spot);
        }
    }
}

// Safety: The rest of this module takes care of never moving out of `WaiterInner` for as long as
// it is part of the wait queue.
unsafe impl linked_list::Linked for WaiterInner {
    type Handle = NonNull<WaiterInner>;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}
