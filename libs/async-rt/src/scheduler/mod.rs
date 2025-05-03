// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod current_thread;
mod multi_thread;

use crate::task::{Header, TaskStub};
use mpsc_queue::MpscQueue;

pub use current_thread::{new_current_thread, CurrentThread};
pub use multi_thread::{new_multi_thread, MultiThread};

#[derive(Debug)]
#[non_exhaustive]
pub struct Tick {
    /// The total number of tasks polled on this scheduler tick.
    pub polled: usize,

    /// The number of polled tasks that *completed* on this scheduler tick.
    ///
    /// This should always be <= `self.polled`.
    pub completed: usize,

    /// `true` if the tick completed with any tasks remaining in the run queue.
    pub has_remaining: bool,
}

/// A single "scheduling instance" (i.e. what actually executes and manages a task)
///
/// In a simple single-core scenario you typically only have a single "scheduling instance", while
/// on multicore scenarios you have one "scheduling instance" per core.
///
/// Note that you can also have multiple scheduling instances on each core if - for example - you
/// want multiple priority levels that preempt each other.
#[derive(Debug)]
struct Core {
    run_queue: MpscQueue<Header>,
}

impl Core {
    pub const DEFAULT_TICK_SIZE: usize = 256;

    const unsafe fn new_with_static_stub(stub: &'static TaskStub) -> Self {
        // Safety: ensured by caller
        unsafe {
            Self {
                run_queue: MpscQueue::new_with_static_stub(&stub.header),
                // current_task: AtomicPtr::new(ptr::null_mut()),
                // queued: AtomicUsize::new(0),
                // spawned: AtomicUsize::new(0),
                // woken: AtomicUsize::new(0),
            }
        }
    }
}
