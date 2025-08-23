// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::device_tree::DeviceTree;
use crate::irq::InterruptController;
use kasync::time::Clock;

#[derive(Debug)]
pub struct Cpu {
    pub id: usize,
    pub clock: Clock,
}

impl Cpu {
    pub fn new(_devtree: &DeviceTree, cpuid: usize) -> crate::Result<Self> {
        // TODO: Initialize x86_64 APIC (Advanced Programmable Interrupt Controller)
        let clock = super::clock::new()?;

        // TODO: Initialize x86_64 CPU from device tree or ACPI tables
        Ok(Self {
            id: cpuid,
            clock,
        })
    }

    pub fn interrupt_controller(&self) -> core::cell::RefMut<'_, dyn InterruptController> {
        todo!();
    }
}
