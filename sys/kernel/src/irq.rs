// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::sync::Arc;
use core::num::NonZero;

use hashbrown::HashMap;
use maitake_sync::wait_queue::WaitQueue;
use spin::{IrqRwLock, LazyLock};

use crate::state::cpu_local;

pub trait InterruptController {
    fn irq_claim(&mut self) -> Option<IrqClaim>;
    fn irq_complete(&mut self, claim: IrqClaim);
    fn irq_mask(&mut self, irq_num: u32);
    fn irq_unmask(&mut self, irq_num: u32);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrqClaim(NonZero<u32>);

impl IrqClaim {
    pub unsafe fn from_raw(raw: NonZero<u32>) -> Self {
        Self(raw)
    }
    pub fn as_u32(self) -> u32 {
        self.0.get()
    }
}

// hashbrown doesn't have a good const constructor, therefore the `LazyLock`
static QUEUES: LazyLock<IrqRwLock<HashMap<u32, Arc<WaitQueue>>>> =
    LazyLock::new(|| IrqRwLock::new(HashMap::new()));

pub fn trigger_irq(irq_ctl: &mut dyn InterruptController) {
    let Some(claim) = irq_ctl.irq_claim() else {
        // Spurious interrupt
        return;
    };

    // acknowledge the interrupt as fast as possible
    irq_ctl.irq_complete(claim);

    let Some(queues) = QUEUES.try_read() else {
        log::warn!("couldn't acquire QUEUES read lock!");
        return;
    };

    if let Some(queue) = queues.get(&claim.as_u32()) {
        queue.wake_all();
    }
}

pub async fn next_event(irq_num: u32) -> Result<(), maitake_sync::Closed> {
    // Register the wait entry *before* unmasking.  If we unmasked first, the
    // interrupt could fire between irq_unmask and QUEUES.write(), and
    // trigger_irq would find no queue entry and drop the wakeup.  Worse, if
    // the interrupt arrived while we held QUEUES.write(), trigger_irq would
    // spin on QUEUES.read() while we can't release the write guard — deadlock.
    let wait = {
        let mut queues = QUEUES.write();
        let wait = queues
            .entry(irq_num)
            .or_insert_with(|| Arc::new(WaitQueue::new()))
            .wait_owned();
        // don't hold the RwLock guard across the await point
        drop(queues);
        wait
    };

    // Unmask only after the write guard is dropped.  Any interrupt that fires
    // from here will find the queue entry and call wake_all(); the WaitQueue
    // stores the notification in its state so the .await below returns
    // immediately even if the wakeup races with the first poll.
    cpu_local()
        .arch
        .cpu
        .interrupt_controller()
        .irq_unmask(irq_num);

    let res = wait.await;

    cpu_local()
        .arch
        .cpu
        .interrupt_controller()
        .irq_mask(irq_num);

    res
}
