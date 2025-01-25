// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::Mutex;
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
    pub const fn new(n: usize) -> Self {
        Self {
            lock: Mutex::new(BarrierState {
                count: 0,
                generation_id: 0,
            }),
            num_threads: n,
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
