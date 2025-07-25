// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::sync::Arc;
use core::num::NonZero;

use hashbrown::HashMap;
use kasync::sync::wait_queue::WaitQueue;
use spin::{LazyLock, RwLock};

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
static QUEUES: LazyLock<RwLock<HashMap<u32, Arc<WaitQueue>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn trigger_irq(irq_ctl: &mut dyn InterruptController) {
    let Some(claim) = irq_ctl.irq_claim() else {
        // Spurious interrupt
        return;
    };

    // acknowledge the interrupt as fast as possible
    irq_ctl.irq_complete(claim);

    let queues = QUEUES.read();
    if let Some(queue) = queues.get(&claim.as_u32()) {
        tracing::trace!("waking wakers for irq-{}", claim.as_u32());
        let woken = queue.wake_all();
        tracing::trace!("woke {woken} wakers for irq-{}", claim.as_u32());
    }
}

pub async fn next_event(irq_num: u32) -> Result<(), kasync::Closed> {
    cpu_local()
        .arch
        .cpu
        .interrupt_controller()
        .irq_unmask(irq_num);

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

    let res = wait.await;

    cpu_local()
        .arch
        .cpu
        .interrupt_controller()
        .irq_mask(irq_num);

    res
}
