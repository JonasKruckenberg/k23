// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use crate::scheduler2::scheduler::queue::Overflow;
use crate::scheduler2::task;
use crate::scheduler2::task::{OwnedTasks, TaskRef};
use core::sync::atomic::AtomicBool;
use sync::Mutex;
use worker::Shared;

mod idle;
mod queue;
pub mod worker;

pub use worker::Context;

#[allow(tail_expr_drop_order)]
pub fn create(num_cores: usize) -> Handle {
    let mut cores = Vec::with_capacity(num_cores);
    let mut remotes = Vec::with_capacity(num_cores);

    for i in 0..num_cores {
        let (steal, run_queue) = queue::new();

        cores.push(Box::new(worker::Core {
            index: i,
            lifo_slot: None,
            run_queue,
            is_searching: false,
        }));

        remotes.push(worker::Remote {
            steal,
        });
    }


    let (idle_shared, idle_synced) = idle::Idle::new(cores, num_cores);

    Handle {
        shared: Shared {
            remotes: remotes.into_boxed_slice(),
            run_queue: RunQueue::new(),
            owned: OwnedTasks::new(),
            idle: idle_shared,
            synced: Mutex::new(worker::Synced {
                assigned_cores: vec![],
                shutdown_cores: vec![],
                idle: idle_synced,
            }),
        },
    }
}

pub struct Handle {
    shared: Shared,
}

impl task::Schedule for Handle {
    fn schedule(&self, task: TaskRef) {
        self.shared.schedule_task(task);
    }

    fn current_task(&self) -> Option<TaskRef> {
        self.shared.current_task()
    }

    fn release(&self, task: TaskRef) -> Option<TaskRef> {
        todo!()
    }

    fn yield_now(&self, task: TaskRef) {
        todo!()
    }
}

struct RunQueue {
    inner: mpsc_queue::MpscQueue<task::raw::Header>,
    is_closed: AtomicBool,
}

impl RunQueue {
    fn new() -> Self {
        Self {
            inner: mpsc_queue::MpscQueue::new_with_stub(TaskRef::new_stub()),
            is_closed: AtomicBool::new(false)
        }
    }
    pub(crate) fn pop(&self) -> Option<TaskRef> {
        self.inner.dequeue()
    }
}

impl Overflow for RunQueue {
    fn push(&self, task: TaskRef) {
        self.inner.enqueue(task);
    }

    fn push_batch<I>(&self, iter: I)
    where
        I: Iterator<Item = TaskRef>,
    {
        for task in iter {
            self.inner.enqueue(task);
        }
    }
}
