// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Coordinates idling workers

use super::worker;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use sync::MutexGuard;

pub(crate) struct Idle {
    /// Number of searching cores
    num_searching: AtomicUsize,
    /// Number of idle cores
    num_idle: AtomicUsize,
    /// Map of idle cores
    idle_map: IdleMap,
    /// Used to catch false-negatives when waking workers
    needs_searching: AtomicBool,
    /// Total number of cores
    num_cores: usize,
}

pub(crate) struct IdleMap {
    chunks: Vec<AtomicUsize>,
}

pub(crate) struct Snapshot {
    chunks: Vec<usize>,
}

/// Data synchronized by the scheduler mutex
pub(crate) struct Synced {
    /// CPU IDs that are currently sleeping
    sleepers: Vec<usize>,

    /// Cores available for workers
    #[expect(clippy::vec_box, reason = "we're moving the boxed core around")]
    available_cores: Vec<Box<worker::Core>>,
}

impl Idle {
    #[expect(clippy::vec_box, reason = "we're moving the boxed core around")]
    pub(crate) fn new(cores: Vec<Box<worker::Core>>) -> (Idle, Synced) {
        let idle = Idle {
            num_searching: AtomicUsize::new(0),
            num_idle: AtomicUsize::new(cores.len()),
            idle_map: IdleMap::new(&cores),
            needs_searching: AtomicBool::new(false),
            num_cores: cores.len(),
        };

        let synced = Synced {
            sleepers: Vec::with_capacity(cores.len()),
            available_cores: cores,
        };

        (idle, synced)
    }

    pub(crate) fn num_searching(&self) -> usize {
        self.num_searching.load(Ordering::Acquire)
    }

    pub(crate) fn num_idle(&self, synced: &Synced) -> usize {
        debug_assert_eq!(
            synced.available_cores.len(),
            self.num_idle.load(Ordering::Acquire)
        );
        synced.available_cores.len()
    }

    pub(crate) fn needs_searching(&self) -> bool {
        self.needs_searching.load(Ordering::Acquire)
    }

    pub(crate) fn try_acquire_available_core(
        &self,
        synced: &mut Synced,
    ) -> Option<Box<worker::Core>> {
        let ret = synced.available_cores.pop();

        if let Some(core) = &ret {
            // Decrement the number of available cores
            let num_idle = self.num_idle.load(Ordering::Acquire) - 1;
            debug_assert_eq!(num_idle, synced.available_cores.len());
            self.num_idle.store(num_idle, Ordering::Release);

            // And remove the worker from the idle map
            self.idle_map.unset(core.index);
            debug_assert!(self.idle_map.matches(&synced.available_cores));
        }

        ret
    }

    /// The worker releases the given core, making it available to other workers
    /// that are waiting.
    pub(crate) fn release_core(&self, synced: &mut worker::Synced, core: Box<worker::Core>) {
        // The core should not be searching at this point
        debug_assert!(!core.is_searching);
        // Check that there are no pending tasks in the global queue
        // debug_assert!(synced.inject.is_empty());

        // Sanity check that the number of available cores matches the number of idle workers
        let num_idle = synced.idle.available_cores.len();
        debug_assert_eq!(num_idle, self.num_idle.load(Ordering::Acquire));

        // Add the worker to the idle map
        self.idle_map.set(core.index);

        // Store the core in the list of available cores
        synced.idle.available_cores.push(core);

        debug_assert!(self.idle_map.matches(&synced.idle.available_cores));

        // Increment the number of idle workers
        self.num_idle.store(num_idle + 1, Ordering::Release);
    }

    /// Wakes up a single worker. This method is intended to be called from a worker cpu.
    pub(crate) fn notify_local(&self, shared: &worker::Shared) {
        if self.num_searching.load(Ordering::Acquire) != 0 {
            // There already is a searching cpu. Note, that this could be a
            // false positive. However, because this method is called **from** a
            // cpu, we know that there is at least one worker currently
            // awake, so the scheduler won't deadlock.
            return;
        }

        // TODO why??
        if self.num_idle.load(Ordering::Acquire) == 0 {
            self.needs_searching.store(true, Ordering::Release);
            return;
        }

        // There aren't any searching workers. Try to initialize one
        if self
            .num_searching
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Failing the compare_exchange means another thread concurrently
            // launched a searching worker.
            return;
        }

        // Acquire the lock
        let synced = shared.synced.lock();
        self.notify_synced(synced, shared);
    }

    /// Wakes up a single worker. This method can be used from any cpu, even from outside worker
    /// cpus.
    pub(crate) fn notify_remote(
        &self,
        synced: MutexGuard<'_, worker::Synced>,
        shared: &worker::Shared,
    ) {
        // TODO why??
        if synced.idle.sleepers.is_empty() {
            self.needs_searching.store(true, Ordering::Release);
            return;
        }

        // We need to establish a stronger barrier than with `notify_local`
        self.num_searching.fetch_add(1, Ordering::AcqRel);

        self.notify_synced(synced, shared);
    }

    fn notify_synced(&self, mut synced: MutexGuard<'_, worker::Synced>, shared: &worker::Shared) {
        // Find a sleeping worker
        if let Some(worker) = synced.idle.sleepers.pop() {
            // Find an available core
            if let Some(mut core) = self.try_acquire_available_core(&mut synced.idle) {
                debug_assert!(!core.is_searching);
                core.is_searching = true;

                // Assign the core to the worker
                synced.assigned_cores[worker] = Some(core);

                // Drop the lock before notifying the condvar.
                drop(synced);

                // super::counters::inc_num_unparks_remote();

                // Notify the worker
                shared.condvars[worker].notify_one(&shared.parking_spot);
                return;
            } else {
                // We didn't find a core for this worker, so push it back into the list of sleepers
                // for next time.
                synced.idle.sleepers.push(worker);
            }
        }

        // If we reach this point, there were either no sleeping workers or no available cores.
        // TODO what does this mean??

        self.needs_searching.store(true, Ordering::Release);
        self.num_searching.fetch_sub(1, Ordering::Release);

        // Explicit mutex guard drop to show that holding the guard to this
        // point is significant. `needs_searching` and `num_searching` must be
        // updated in the critical section.
        drop(synced);
    }

    pub(crate) fn notify_many(
        &self,
        synced: &mut MutexGuard<worker::Synced>,
        workers: &mut Vec<usize>,
        num: usize,
    ) {
        debug_assert!(workers.is_empty());

        for _ in 0..num {
            if let Some(worker) = synced.idle.sleepers.pop() {
                if let Some(core) = synced.idle.available_cores.pop() {
                    debug_assert!(!core.is_searching);

                    self.idle_map.unset(core.index);

                    synced.assigned_cores[worker] = Some(core);

                    workers.push(worker);

                    continue;
                } else {
                    synced.idle.sleepers.push(worker);
                }
            }
        }

        if !workers.is_empty() {
            debug_assert!(self.idle_map.matches(&synced.idle.available_cores));
            let num_idle = synced.idle.available_cores.len();
            self.num_idle.store(num_idle, Ordering::Release);
        } else {
            debug_assert_eq!(
                synced.idle.available_cores.len(),
                self.num_idle.load(Ordering::Acquire)
            );
            self.needs_searching.store(true, Ordering::Release);
        }
    }

    pub(crate) fn shutdown(&self, synced: &mut worker::Synced, shared: &worker::Shared) {
        // Wake every sleeping worker and assign a core to it. There may not be
        // enough sleeping workers for all cores, but other workers will
        // eventually find the cores and shut them down.
        while !synced.idle.sleepers.is_empty() && !synced.idle.available_cores.is_empty() {
            let worker = synced.idle.sleepers.pop().unwrap();
            let core = self.try_acquire_available_core(&mut synced.idle).unwrap();

            synced.assigned_cores[worker] = Some(core);
            log::trace!("waking sleeping worker {worker} for shutdown...");
            shared.condvars[worker].notify_one(&shared.parking_spot);
        }

        debug_assert!(self.idle_map.matches(&synced.idle.available_cores));

        // Wake up any other workers
        while let Some(worker) = synced.idle.sleepers.pop() {
            log::trace!("waking worker {worker} for shutdown...");
            shared.condvars[worker].notify_one(&shared.parking_spot);
        }
    }

    pub(crate) fn shutdown_unassigned_cores(&self, shared: &worker::Shared) {
        // If there are any remaining cores, shut them down here.
        //
        // This code is a bit convoluted to avoid lock-reentry.
        while let Some(core) = {
            let mut synced = shared.synced.lock();
            self.try_acquire_available_core(&mut synced.idle)
        } {
            shared.shutdown_core(core);
        }
    }

    pub(crate) fn transition_worker_to_parked(&self, synced: &mut worker::Synced, index: usize) {
        // Store the worker index in the list of sleepers
        synced.idle.sleepers.push(index);

        // The worker's assigned core slot should be empty
        debug_assert!(synced.assigned_cores[index].is_none());
    }

    pub(crate) fn try_transition_worker_to_searching(&self, core: &mut worker::Core) {
        debug_assert!(!core.is_searching);

        let num_searching = self.num_searching.load(Ordering::Acquire);
        let num_idle = self.num_idle.load(Ordering::Acquire);

        if 2 * num_searching >= self.num_cores - num_idle {
            return;
        }

        self.transition_worker_to_searching(core);
    }

    pub(crate) fn transition_worker_to_searching(&self, core: &mut worker::Core) {
        core.is_searching = true;
        self.num_searching.fetch_add(1, Ordering::AcqRel);
        self.needs_searching.store(false, Ordering::Release);
    }

    pub(crate) fn transition_worker_from_searching(&self) -> bool {
        let prev = self.num_searching.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0);

        prev == 1
    }
}

impl IdleMap {
    fn new(cores: &[Box<worker::Core>]) -> IdleMap {
        let ret = IdleMap::new_n(num_chunks(cores.len()));
        ret.set_all(cores);

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

    /// Mark all cores as idle
    fn set_all(&self, cores: &[Box<worker::Core>]) {
        for core in cores {
            self.set(core.index);
        }
    }

    /// Unmark a specific core as idle
    fn unset(&self, index: usize) {
        let (chunk, mask) = index_to_mask(index);
        let prev = self.chunks[chunk].load(Ordering::Acquire);
        let next = prev & !mask;
        self.chunks[chunk].store(next, Ordering::Release);
    }

    /// Check if the given cores are idle
    fn matches(&self, idle_cores: &[Box<worker::Core>]) -> bool {
        let expect = IdleMap::new_n(self.chunks.len());
        expect.set_all(idle_cores);

        for (i, chunk) in expect.chunks.iter().enumerate() {
            if chunk.load(Ordering::Acquire) != self.chunks[i].load(Ordering::Acquire) {
                return false;
            }
        }

        true
    }
}

impl Snapshot {
    pub(crate) fn new(idle: &Idle) -> Snapshot {
        let chunks = vec![0; idle.idle_map.chunks.len()];
        let mut ret = Snapshot { chunks };
        ret.update(&idle.idle_map);
        ret
    }

    fn update(&mut self, idle_map: &IdleMap) {
        for i in 0..self.chunks.len() {
            self.chunks[i] = idle_map.chunks[i].load(Ordering::Acquire);
        }
    }

    pub(crate) fn is_idle(&self, index: usize) -> bool {
        let (chunk, mask) = index_to_mask(index);
        debug_assert!(
            chunk < self.chunks.len(),
            "index={}; chunks={}",
            index,
            self.chunks.len()
        );
        self.chunks[chunk] & mask == mask
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
