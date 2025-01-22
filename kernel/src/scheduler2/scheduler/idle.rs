// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use sync::MutexGuard;
use crate::scheduler2::scheduler::worker;

pub(super) struct Idle {
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

pub(super) struct IdleMap {
    chunks: Vec<AtomicUsize>,
}

pub(super) struct Snapshot {
    chunks: Vec<usize>,
}

/// Data synchronized by the scheduler mutex
pub(super) struct Synced {
    /// Worker IDs that are currently sleeping
    sleepers: Vec<usize>,

    /// Cores available for workers
    available_cores: Vec<Box<worker::Core>>,
}

impl Idle {
    pub(super) fn new(cores: Vec<Box<worker::Core>>, num_workers: usize) -> (Idle, Synced) {
        let idle = Idle {
            num_searching: AtomicUsize::new(0),
            num_idle: AtomicUsize::new(cores.len()),
            idle_map: IdleMap::new(&cores),
            needs_searching: AtomicBool::new(false),
            num_cores: cores.len(),
        };

        let synced = Synced {
            sleepers: Vec::with_capacity(num_workers),
            available_cores: cores,
        };

        (idle, synced)
    }
    
    pub(super) fn needs_searching(&self) -> bool {
        self.needs_searching.load(Ordering::Acquire)
    }

    pub(super) fn num_idle(&self, synced: &Synced) -> usize {
        debug_assert_eq!(synced.available_cores.len(), self.num_idle.load(Ordering::Acquire));
        synced.available_cores.len()
    }

    /// Try to acquire an available core
    pub(super) fn try_acquire_available_core(&self, synced: &mut Synced) -> Option<Box<worker::Core>> {
        let ret = synced.available_cores.pop();

        if let Some(core) = &ret {
            // Decrement the number of idle cores
            let num_idle = self.num_idle.load(Ordering::Acquire) - 1;
            debug_assert_eq!(num_idle, synced.available_cores.len());
            self.num_idle.store(num_idle, Ordering::Release);

            self.idle_map.unset(core.index);
            debug_assert!(self.idle_map.matches(&synced.available_cores));
        }

        ret
    }

    pub(super) fn notify_local(&self, shared: &worker::Shared) {
        if self.num_searching.load(Ordering::Acquire) != 0 {
            // There already is a searching worker. Note, that this could be a
            // false positive. However, because this method is called **from** a
            // worker, we know that there is at least one worker currently
            // awake, so the scheduler won't deadlock.
            return;
        }

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

        // super::counters::inc_num_unparks_local();

        // Acquire the lock
        let synced = shared.synced.lock();
        self.notify_synced(synced, shared);
    }

    /// Notifies a single worker
    pub(super) fn notify_remote(&self, synced: MutexGuard<'_, worker::Synced>, shared: &worker::Shared) {
        if synced.idle.sleepers.is_empty() {
            self.needs_searching.store(true, Ordering::Release);
            return;
        }

        // We need to establish a stronger barrier than with `notify_local`
        self.num_searching.fetch_add(1, Ordering::AcqRel);

        self.notify_synced(synced, shared);
    }

    /// Notify a worker while synced
    fn notify_synced(&self, mut synced: MutexGuard<'_, worker::Synced>, _shared: &worker::Shared) {
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
                riscv::sbi::ipi::send_ipi(1 << worker, 0).unwrap();

                return;
            } else {
                synced.idle.sleepers.push(worker);
            }
        }

        // super::counters::inc_notify_no_core();

        // Set the `needs_searching` flag, this happens *while* the lock is held.
        self.needs_searching.store(true, Ordering::Release);
        self.num_searching.fetch_sub(1, Ordering::Release);

        // Explicit mutex guard drop to show that holding the guard to this
        // point is significant. `needs_searching` and `num_searching` must be
        // updated in the critical section.
        drop(synced);
    }

    pub(super) fn notify_mult(
        &self,
        synced: &mut worker::Synced,
        workers: &mut Vec<usize>,
        num: usize,
    ) {
        debug_assert!(workers.is_empty());

        for _ in 0..num {
            if let Some(worker) = synced.idle.sleepers.pop() {
                // TODO: can this be switched to use next_available_core?
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

            break;
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

    pub(super) fn transition_worker_to_parked(&self, synced: &mut worker::Synced, index: usize) {
        // Store the worker index in the list of sleepers
        synced.idle.sleepers.push(index);

        // The worker's assigned core slot should be empty
        debug_assert!(synced.assigned_cores[index].is_none());
    }

    pub(super) fn transition_worker_to_searching(&self, core: &mut worker::Core) {
        core.is_searching = true;
        self.num_searching.fetch_add(1, Ordering::AcqRel);
        self.needs_searching.store(false, Ordering::Release);
    }

    /// A lightweight transition from searching -> running.
    ///
    /// Returns `true` if this is the final searching worker. The caller
    /// **must** notify a new worker.
    pub(super) fn transition_worker_from_searching(&self) -> bool {
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

    fn set(&self, index: usize) {
        let (chunk, mask) = index_to_mask(index);
        let prev = self.chunks[chunk].load(Ordering::Acquire);
        let next = prev | mask;
        self.chunks[chunk].store(next, Ordering::Release);
    }

    fn set_all(&self, cores: &[Box<worker::Core>]) {
        for core in cores {
            self.set(core.index);
        }
    }

    fn unset(&self, index: usize) {
        let (chunk, mask) = index_to_mask(index);
        let prev = self.chunks[chunk].load(Ordering::Acquire);
        let next = prev & !mask;
        self.chunks[chunk].store(next, Ordering::Release);
    }

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
