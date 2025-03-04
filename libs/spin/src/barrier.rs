// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::Mutex;
use crate::loom::loom_const_fn;
use core::hint;

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
        pub fn new(n: usize) -> Self {
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

            while local_gen == lock.generation_id && lock.count < self.num_threads {
                drop(lock);
                hint::spin_loop();
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
    use super::*;
    use crate::loom::Arc;
    use crate::loom::thread;
    use std::sync::mpsc::{TryRecvError, channel};

    #[test]
    fn test_barrier() {
        const N: usize = 10;

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

        let mut leader_found = barrier.wait().is_leader();

        // Now, the barrier is cleared and we should get data.
        for _ in 0..N - 1 {
            if rx.recv().unwrap() {
                assert!(!leader_found);
                leader_found = true;
            }
        }
        assert!(leader_found);
    }
}
