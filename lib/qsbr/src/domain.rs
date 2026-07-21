// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr::NonNull;

use cordyceps::{list, Stack, TransferStack};
use spin::Mutex;

use crate::loom::sync::atomic::{fence, AtomicU64, Ordering};
use crate::reader::IDLE_STATE;
use crate::{QsbrHead, QsbrReader};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epoch(pub(crate) u64);

pub struct QsbrDomain {
    pub(crate) global_epoch: AtomicU64,
    pub(crate) cpus: Mutex<list::List<QsbrReader>>,
    retired: TransferStack<QsbrHead>,
}

// SAFETY: `QsbrCpu` is `!Send + !Sync` to keep its *public* API
// context-bound (guards minted by one CPU must not be usable from another),
// which strips the registry's structural auto-traits. Cross-context access
// by this crate is confined to each CPU's atomic epoch word and, under the
// registry mutex, its intrusive links — both race-free.
unsafe impl Send for QsbrDomain {}
// SAFETY: as above.
unsafe impl Sync for QsbrDomain {}

impl QsbrDomain {
    pub const fn new() -> Self {
        Self {
            global_epoch: AtomicU64::new(1),
            cpus: Mutex::new(list::List::new()),
            retired: TransferStack::new(),
        }
    }

    pub fn advance(&self) -> Epoch {
        Epoch(self.global_epoch.fetch_add(1, Ordering::Release))
    }

    pub fn poll(&self, epoch: Epoch) -> bool {
        epoch.0 < self.oldest_active_epoch()
    }

    pub unsafe fn retire(&self, node: NonNull<QsbrHead>) {
        let epoch = self.global_epoch.fetch_add(1, Ordering::Release);
        // SAFETY: exclusive access to the unqueued node per contract; raw
        // write, no reference formed.
        unsafe { (*node.as_ptr()).epoch = epoch };
        self.retired.push(node);
    }

    pub fn reclaim(&self, budget: usize) -> usize {
        let oldest_active_epoch = self.oldest_active_epoch();

        let mut ready: Stack<QsbrHead> = Stack::new();

        for (n, node) in self.retired.take_all().into_iter().enumerate() {
            // SAFETY: node was queued by `retire`, whose contract keeps it
            // valid until its `drop_fn` runs.
            let _node = unsafe { node.as_ref() };

            if n < budget && _node.epoch < oldest_active_epoch {
                ready.push(node);
            } else {
                self.retired.push(node);
            }
        }

        let mut reclaimed = 0;
        while let Some(node) = ready.pop() {
            // SAFETY: as above; additionally its epoch is complete
            // (`epoch < min`), so no CPU still holds a reference, and it
            // was popped, so `drop_fn` runs exactly once.
            let drop_fn = unsafe { node.as_ref() }.drop_fn;
            unsafe { drop_fn(node) };
            reclaimed += 1;
        }
        reclaimed
    }

    /// The earliest epoch any registered, non-idle CPU is still in.
    /// Or `u64::MAX` if there are none. No registered or non-idle CPUs means we can
    /// freely reclaim.
    fn oldest_active_epoch(&self) -> u64 {
        // Pairs with the SeqCst fence in `QsbrCpu::exit_idle` (and thereby
        // registration): if an Acquire load below misses a concurrent epoch
        // store (reads IDLE), the fence pairing guarantees that CPU's
        // subsequent loads see every write the caller made before this call
        // — in particular the unlinking of anything already retired — so
        // skipping it is safe.
        fence(Ordering::SeqCst);

        self.cpus
            .lock()
            .iter()
            .filter_map(|cpu| {
                // Acquire: pairs with the CPU's Release epoch stores; orders
                // its prior critical section before frees made on the strength
                // of this observation.
                let epoch = cpu.state.load(Ordering::Acquire);

                if epoch == IDLE_STATE {
                    None
                } else {
                    Some(epoch)
                }
            })
            .min()
            .unwrap_or(u64::MAX)
    }
}
