// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use k32_util::loom_const_fn;

use crate::{Backoff, Mutex};

pub struct Barrier {
    lock: Mutex<BarrierState>,
    num_threads: usize,
}

// The inner state of a double barrier
struct BarrierState {
    count: usize,
    generation_id: usize,
}

pub struct BarrierWaitResult(bool);

impl Barrier {
    loom_const_fn! {
        pub const fn new(n: usize) -> Self {
            Self {
                lock: Mutex::new(BarrierState {
                    count: 0,
                    generation_id: 0,
                }),
                num_threads: n,
            }
        }
    }

    pub fn wait(&self) -> BarrierWaitResult {
        let mut lock = self.lock.lock();
        lock.count += 1;

        if lock.count < self.num_threads {
            // not the leader
            let local_gen = lock.generation_id;
            let mut boff = Backoff::new();

            while local_gen == lock.generation_id && lock.count < self.num_threads {
                drop(lock);

                boff.spin();

                lock = self.lock.lock();
            }
            BarrierWaitResult(false)
        } else {
            // this thread is the leader,
            //   and is responsible for incrementing the generation
            lock.count = 0;
            lock.generation_id = lock.generation_id.wrapping_add(1);
            BarrierWaitResult(true)
        }
    }
}

impl BarrierWaitResult {
    pub fn is_leader(&self) -> bool {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::TryRecvError;

    use super::*;
    use crate::loom;
    use crate::loom::sync::Arc;
    use crate::loom::sync::mpsc::channel;
    use crate::loom::thread;

    #[test]
    fn test_barrier() {
        loom::model(|| {
            const N: usize = loom::MAX_THREADS;

            let barrier = Arc::new(Barrier::new(N));
            let (tx, rx) = channel();

            for _ in 0..N - 1 {
                let c = barrier.clone();
                let tx = tx.clone();
                thread::spawn(move || {
                    tx.send(c.wait().is_leader()).unwrap();
                });
            }

            // At this point, all spawned threads should be blocked,
            // so we shouldn't get anything from the port
            assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

            #[cfg(loom)]
            crate::loom::thread::yield_now();

            let mut leader_found = barrier.wait().is_leader();

            // Now, the barrier is cleared and we should get data.
            for _ in 0..N - 1 {
                if rx.recv().unwrap() {
                    assert!(!leader_found);
                    leader_found = true;
                }
            }
            assert!(leader_found);
        });
    }
}
