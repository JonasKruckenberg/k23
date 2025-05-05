// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(all(not(test), target_os = "none"), no_std)]
#![feature(thread_local)]
#![feature(never_type)]
#![feature(allocator_api)]

extern crate alloc;

use crate::task::{Header, TaskRef, TaskStub};
use cpu_local::cpu_local;
use fastrand::FastRand;
use mpsc_queue::MpscQueue;
use spin::Backoff;

// pub mod scheduler;
pub mod task;
mod time;

pub struct Tick {
    /// The total number of tasks polled on this scheduler tick.
    pub polled: usize,

    /// The number of polled tasks that *completed* on this scheduler tick.
    ///
    /// This should always be <= `self.polled`.
    pub completed: usize,

    /// `true` if the tick completed with any tasks remaining in the run queue.
    pub has_remaining: bool,

    /// The number of tasks that were woken from outside of their own `poll`
    /// calls since the last tick.
    pub woken_external: usize,

    /// The number of tasks that were woken from within their own `poll` calls
    /// during this tick.
    pub woken_internal: usize,
}

cpu_local! {
    static CORE: Core = const {
        static STUB_TASK: TaskStub = TaskStub::new();
        unsafe { Core::new_with_static_stub(&STUB_TASK) }
    };
}

struct Core {
    run_queue: MpscQueue<Header>,
    lifo_slot: Option<TaskRef>,
}

impl Core {
    const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
        Core {
            // Safety: ensured by caller
            run_queue: unsafe { MpscQueue::new_with_static_stub(&stub.header) },
            lifo_slot: None,
        }
    }

    fn tick_n(&self, n: usize) -> Tick {
        todo!()
    }
}

struct Worker {
    rng: FastRand,
}

impl Worker {
    pub fn tick_n(&mut self, n: usize) -> Tick {
        CORE.with(|core| {
            let mut tick = core.tick_n(n);

            // TODO this is a great time to drive the timer too

            // the scheduler still has tasks in its queue, which means it hit its `n` limit,
            // and we should yield back to the caller
            if tick.has_remaining {
                return tick;
            }

            // at this point, the scheduler is out of tasks
            // we should try to "find" some more from other cores if possible
            let stolen = self.try_steal(core);
            if stolen > 0 {
                tracing::debug!(tick.stolen = stolen);
                // if we stole tasks, we can continue to execute tasks and need to signal that to our caller!
                tick.has_remaining = true;
                return tick;
            }

            // we haven't found any tasks, `has_remaining = false` will signal our caller to go to sleep...
            tick
        })
    }

    fn try_steal(&mut self, core: &Core) -> usize {
        debug_assert!(core.lifo_slot.is_none());
        
        const ROUNDS: usize = 4;

        let num_workers = 0; // TODO how do get this number???
        let mut backoff = Backoff::new();

        for _ in 0..ROUNDS {
            // Start from a random worker
            let start = self.rng.fastrand_n(num_workers) as usize;

            backoff.spin();
        }

        0
    }
}
