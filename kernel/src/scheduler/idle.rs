// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use sync::Mutex;

pub struct Idle {
    /// Number of searching workers
    num_stealing: AtomicUsize,
    /// Number of idle workers
    num_idle: AtomicUsize,
    /// Map of idle cores
    idle_map: IdleMap,
    /// Total number of cores
    num_cores: usize,
    /// Worker IDs that are currently sleeping
    sleepers: Mutex<Vec<usize>>,
}

pub(crate) struct IdleMap {
    chunks: Vec<AtomicUsize>,
}

impl Idle {
    pub fn new(num_cores: usize) -> Self {
        Self {
            num_stealing: AtomicUsize::new(0),
            num_idle: AtomicUsize::new(0),
            idle_map: IdleMap::new(num_cores),
            num_cores,
            sleepers: Mutex::new(Vec::with_capacity(num_cores)),
        }
    }

    pub fn transition_worker_to_waiting(&self, worker: &super::Worker) {
        // log::trace!("Idle::transition_worker_to_waiting");

        // The worker should not be stealing at this point
        debug_assert!(!worker.is_stealing);
        // Check that there are no pending tasks in the global queue
        debug_assert!(worker.scheduler.run_queue.is_empty());

        self.idle_map.set(worker.cpuid);

        // Update `num_idle`
        let prev = self.num_idle.fetch_add(1, Ordering::Release);
        debug_assert!(prev < self.num_cores);

        // Store the worker index in the list of sleepers
        self.sleepers.lock().push(worker.cpuid);
    }

    pub fn transition_worker_from_waiting(&self, worker: &super::Worker) {
        // log::trace!("Idle::transition_worker_from_waiting");

        // Decrement the number of idle cores
        let prev = self.num_idle.fetch_sub(1, Ordering::Acquire);
        debug_assert!(prev > 0);

        self.idle_map.unset(worker.cpuid);

        self.sleepers
            .lock()
            .retain(|sleeper| *sleeper != worker.cpuid);
    }

    pub fn try_transition_worker_to_stealing(&self, worker: &mut super::Worker) {
        // log::trace!("Idle::try_transition_worker_to_stealing");

        debug_assert!(!worker.is_stealing);

        let num_searching = self.num_stealing.load(Ordering::Acquire);
        let num_idle = self.num_idle.load(Ordering::Acquire);

        if 2 * num_searching >= self.num_cores - num_idle {
            return;
        }

        worker.is_stealing = true;
        self.num_stealing.fetch_add(1, Ordering::AcqRel);
    }

    /// A lightweight transition from stealing -> running.
    ///
    /// Returns `true` if this is the final searching worker. The caller
    /// **must** notify a new worker.
    pub fn transition_worker_from_stealing(&self) -> bool {
        // log::trace!("Idle::transition_worker_from_stealing");

        let prev = self.num_stealing.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0);

        prev == 1
    }

    pub fn notify_one(&self) {
        // log::trace!("Idle::notify_one");
        if let Some(worker) = self.sleepers.lock().pop() {
            // Safety: the worker placed itself into the sleepers list, so sending a wakeup is safe
            unsafe {
                arch::cpu_unpark(worker);
            }
        }
    }

    pub fn notify_all(&self) {
        // log::trace!("Idle::notify_all");
        while let Some(worker) = self.sleepers.lock().pop() {
            // Safety: the worker placed itself into the sleepers list, so sending a wakeup is safe
            unsafe {
                arch::cpu_unpark(worker);
            }
        }
    }
}

impl IdleMap {
    fn new(num_cores: usize) -> IdleMap {
        let ret = IdleMap::new_n(num_chunks(num_cores));
        for index in 0..num_cores {
            ret.set(index);
        }

        ret
    }

    fn new_n(n: usize) -> IdleMap {
        let chunks = (0..n).map(|_| AtomicUsize::new(0)).collect();
        IdleMap { chunks }
    }

    /// Mark a specific core as idle
    fn set(&self, index: usize) {
        let (chunk, mask) = index_to_mask(index);
        let prev = self.chunks[chunk].load(Ordering::Acquire);
        let next = prev | mask;
        self.chunks[chunk].store(next, Ordering::Release);
    }

    fn unset(&self, index: usize) {
        let (chunk, mask) = index_to_mask(index);
        let prev = self.chunks[chunk].load(Ordering::Acquire);
        let next = prev & !mask;
        self.chunks[chunk].store(next, Ordering::Release);
    }
}

const BITS: usize = usize::BITS as usize;
const BIT_MASK: usize = (usize::BITS - 1) as usize;

fn num_chunks(max_cores: usize) -> usize {
    (max_cores / BITS) + 1
}

fn index_to_mask(index: usize) -> (usize, usize) {
    let mask = 1 << (index & BIT_MASK);
    let chunk = index / BITS;

    (chunk, mask)
}
