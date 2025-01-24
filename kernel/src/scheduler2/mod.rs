// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod idle;
mod queue;
mod task;
pub mod worker;
mod yield_now;

use crate::scheduler2::idle::Idle;
use crate::scheduler2::task::{JoinHandle, OwnedTasks, TaskRef};
use crate::scheduler2::worker::{Core, Remote, Shared, Synced};
use crate::thread_local::ThreadLocal;
use crate::util::condvar::Condvar;
use crate::util::fast_rand::FastRand;
use crate::util::parking_spot::ParkingSpot;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::future::Future;
use rand::RngCore;
use sync::{Mutex, OnceLock};

static SCHEDULER: OnceLock<Handle> = OnceLock::new();

pub fn scheduler() -> &'static Handle {
    SCHEDULER.get().expect("scheduler not initialized")
}

#[cold]
pub fn init(num_cores: usize, rand: &mut impl RngCore) {
    let mut cores = Vec::with_capacity(num_cores);
    let mut remotes = Vec::with_capacity(num_cores);

    for i in 0..num_cores {
        let (steal, run_queue) = queue::new();

        cores.push(Box::new(Core {
            index: i,
            run_queue,
            lifo_slot: None,
            is_searching: false,
            rand: FastRand::new(rand.next_u64()),
        }));
        remotes.push(Remote { steal });
    }

    let (idle, idle_synced) = Idle::new(cores);

    let stub = TaskRef::new_stub();
    let run_queue = mpsc_queue::MpscQueue::new_with_stub(stub);
    #[allow(tail_expr_drop_order)]
    SCHEDULER.get_or_init(|| Handle {
        shared: Shared {
            remotes: remotes.into_boxed_slice(),
            owned: OwnedTasks::new(),
            synced: Mutex::new(Synced {
                assigned_cores: (0..num_cores).map(|_| None).collect(),
                idle: idle_synced,
            }),
            run_queue,
            idle,
            condvars: (0..num_cores).map(|_| Condvar::new()).collect(),
            parking_spot: ParkingSpot::default(),
            tls: ThreadLocal::with_capacity(num_cores),
        },
    });
}

pub struct Handle {
    shared: Shared,
}

impl Handle {
    pub fn spawn<F>(&'static self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let id = task::Id::next();
        let (handle, maybe_task) = self.shared.owned.bind(future, self, id);

        if let Some(task) = maybe_task {
            self.shared.schedule_task(task, false);
        }

        handle
    }
}

impl task::Schedule for &'static Handle {
    fn schedule(&self, task: TaskRef) {
        self.shared.schedule_task(task, false);
    }

    fn release(&self, task: &TaskRef) -> Option<TaskRef> {
        self.shared.owned.remove(task)
    }

    fn yield_now(&self, task: TaskRef) {
        self.shared.schedule_task(task, true);
    }
}
