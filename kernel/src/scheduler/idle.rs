// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

pub struct Idle {
    /// Number of searching workers
    num_searching: AtomicUsize,
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
            num_searching: AtomicUsize::new(0),
            num_idle: AtomicUsize::new(0),
            idle_map: IdleMap::new(num_cores),
            num_cores,
            sleepers: Mutex::new(Vec::with_capacity(num_cores)),
        }
    }

    pub(crate) fn num_searching(&self) -> usize {
        self.num_searching.load(Ordering::Acquire)
    }

    pub fn transition_worker_to_waiting(&self, worker: &super::Worker) {
        // tracing::trace!("Idle::transition_worker_to_waiting");

        // The worker should not be stealing at this point
        debug_assert!(!worker.is_searching);
        // Check that there are no pending tasks in the global queue
        debug_assert!(worker.scheduler.run_queue.is_empty());

        self.idle_map.set(worker.cpuid);

        // Update `num_idle`
        let prev = self.num_idle.fetch_add(1, Ordering::Release);
        debug_assert!(prev < self.num_cores);

        self.sleepers.lock().push(worker.cpuid);
    }

    pub fn transition_worker_from_waiting(&self, worker: &super::Worker) {
        // tracing::trace!("Idle::transition_worker_from_waiting");

        // Decrement the number of idle cores
        let prev = self.num_idle.fetch_sub(1, Ordering::Acquire);
        debug_assert!(prev > 0);

        self.idle_map.unset(worker.cpuid);

        self.sleepers
            .lock()
            .retain(|sleeper| *sleeper != worker.cpuid);
    }

    pub fn try_transition_worker_to_searching(&self, worker: &mut super::Worker) {
        // tracing::trace!("Idle::try_transition_worker_to_searching");

        debug_assert!(!worker.is_searching);

        let num_searching = self.num_searching.load(Ordering::Acquire);
        let num_idle = self.num_idle.load(Ordering::Acquire);

        if 2 * num_searching >= self.num_cores - num_idle {
            return;
        }

        worker.is_searching = true;
        self.num_searching.fetch_add(1, Ordering::AcqRel);
    }

    /// A lightweight transition from searching -> running.
    ///
    /// Returns `true` if this is the final searching worker. The caller
    /// **must** notify a new worker.
    pub fn transition_worker_from_searching(&self) -> bool {
        // tracing::trace!("Idle::transition_worker_from_searching");

        let prev = self.num_searching.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0);

        prev == 1
    }

    pub fn notify_one(&self) {
        // tracing::trace!("Idle::notify_one");
        if let Some(worker) = self.sleepers.lock().pop() {
            // Safety: the worker placed itself into the sleepers list, so sending a wakeup is safe
            unsafe {
                arch::cpu_unpark(worker);
            }
        }
    }

    pub fn notify_all(&self) {
        // tracing::trace!("Idle::notify_all");
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
