// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::{AtomicUsize, Ordering};
use crate::park::parker::Parker;
use crate::park::{Park, UnparkToken};
use crate::time::{Clock, Deadline};
use alloc::vec::Vec;
use spin::Mutex;
use util::loom_const_fn;

#[derive(Debug)]
pub struct ParkingLot<P> {
    /// Number of parked cores
    num_parked: AtomicUsize,
    unpark_tokens: Mutex<Vec<UnparkToken<P>>>,
}

// === impl ParkingLot ===

impl<P: Park + Send + Sync> ParkingLot<P> {
    loom_const_fn! {
        pub const fn new() -> Self {
            Self {
                num_parked: AtomicUsize::new(0),
                unpark_tokens: Mutex::new(Vec::new()),
            }
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            num_parked: AtomicUsize::new(0),
            unpark_tokens: Mutex::new(Vec::with_capacity(capacity)),
        }
    }

    pub fn num_parked(&self) -> usize {
        self.num_parked.load(Ordering::Acquire)
    }

    /// Park the calling execution context using the provided `Parker`.
    ///
    /// Once parked, the execution context will not make progress until unparked through either
    /// `Self::unpark_one` or `Self::unpark_all`.
    pub fn park(&self, parker: Parker<P>) {
        self.transition_to_parked();

        self.unpark_tokens.lock().push(parker.clone().into_unpark());
        parker.park();

        self.transition_from_parked();
    }

    pub fn park_until(&self, parker: Parker<P>, deadline: Deadline, clock: &Clock) {
        self.transition_to_parked();

        self.unpark_tokens.lock().push(parker.clone().into_unpark());
        parker.park_until(deadline, clock);

        self.transition_from_parked();
    }

    /// Unpark a single execution context, blocking if the queue of parked targets is busy.
    /// Returns `true` when a target was unparked and `false` otherwise.
    ///
    /// This method will choose an arbitrary context that has previously parked themselves through
    /// `Self::park`. The order in which individual target are woken is *not defined* and may change
    /// at any point.
    pub fn unpark_one(&self) -> bool {
        if let Some(token) = self.unpark_tokens.lock().pop() {
            token.unpark();
            true
        } else {
            false
        }
    }

    /// Unpark all currently parked execution contexts, returning the number of targets
    /// that were unparked.
    ///
    /// This method will unpark contexts in an arbitrary order, no guarantee is made about specific
    /// ordering and the underlying implementation may change at any point.
    pub fn unpark_all(&self) -> usize {
        let mut tokens = self.unpark_tokens.lock();
        let mut unparked = 0;

        while let Some(token) = tokens.pop() {
            token.unpark();
            unparked += 1;
        }

        unparked
    }

    fn transition_to_parked(&self) {
        // Increment `num_idle` before we park ourselves
        let prev = self.num_parked.fetch_add(1, Ordering::Release);
        assert_ne!(prev, usize::MAX);
    }

    fn transition_from_parked(&self) {
        let prev = self.num_parked.fetch_sub(1, Ordering::Release);
        assert_ne!(prev, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StdPark;
    use crate::loom::sync::{Arc, atomic::AtomicUsize};
    use crate::loom::thread;
    use alloc::vec::Vec;
    use spin::Backoff;

    // FIXME this test deadlocks under loom :/ figure out why and fix
    #[cfg(not(loom))]
    #[test]
    fn parking_lot_basically_works() {
        crate::loom::model(|| {
            crate::loom::lazy_static! {
                static ref UNPARKED: AtomicUsize = AtomicUsize::new(0);
            }

            let lot: Arc<ParkingLot<StdPark>> = Arc::new(ParkingLot::with_capacity(4));

            let joins: Vec<_> = (0..4)
                .map(|_| {
                    let lot = lot.clone();
                    thread::spawn(move || {
                        lot.park(Parker::new(StdPark::for_current()));
                        UNPARKED.fetch_add(1, Ordering::Release);
                    })
                })
                .collect();

            let mut boff = Backoff::new();
            for _ in 0..4 {
                while !lot.unpark_one() {
                    boff.spin();
                }
                boff.reset();
            }

            for join in joins {
                join.join().unwrap();
            }

            assert_eq!(UNPARKED.load(Ordering::Acquire), 4);
        })
    }
}
