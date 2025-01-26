// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod idle;
pub mod worker;

use crate::executor::task::{OwnedTasks, TaskRef};
use crate::executor::{queue, task, JoinHandle};
use crate::thread_local::ThreadLocal;
use crate::util::condvar::Condvar;
use crate::util::fast_rand::FastRand;
use crate::util::parking_spot::ParkingSpot;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::future::Future;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Waker;
use idle::Idle;
use rand::RngCore;
use sync::Mutex;
use worker::{Core, Remote, Shared, Synced};

pub struct Handle {
    shared: Shared,
}

impl Handle {
    #[expect(tail_expr_drop_order, reason = "")]
    pub fn new(num_cores: usize, rand: &mut impl RngCore, shutdown_on_idle: bool) -> Self {
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

        Self {
            shared: Shared {
                shutdown: AtomicBool::new(false),
                remotes: remotes.into_boxed_slice(),
                owned: OwnedTasks::new(),
                synced: Mutex::new(Synced {
                    assigned_cores: (0..num_cores).map(|_| None).collect(),
                    idle: idle_synced,
                    shutdown_cores: Vec::with_capacity(num_cores),
                }),
                run_queue,
                idle,
                condvars: (0..num_cores).map(|_| Condvar::new()).collect(),
                parking_spot: ParkingSpot::default(),
                tls: ThreadLocal::with_capacity(num_cores),
                shutdown_on_idle,
            },
        }
    }

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

    pub fn shutdown(&self) {
        if !self.shared.shutdown.swap(true, Ordering::AcqRel) {
            let mut synced = self.shared.synced.lock();

            // Set the shutdown flag on all available cores
            self.shared.idle.shutdown(&mut synced, &self.shared);

            // Any unassigned cores need to be shutdown, but we have to first drop
            // the lock
            drop(synced);
            self.shared.idle.shutdown_unassigned_cores(&self.shared);
        }
    }

    #[inline]
    pub(in crate::executor) fn defer(&self, waker: &Waker) {
        self.shared.tls.get().unwrap().defer(waker);
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
