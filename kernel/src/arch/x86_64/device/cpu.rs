// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::device_tree::DeviceTree;
use crate::irq::InterruptController;
use core::time::Duration;
use kasync::time::{Clock, NANOS_PER_SEC, Ticks};

#[derive(Debug)]
pub struct Cpu {
    pub id: usize,
    pub clock: Clock,
}

impl Cpu {
    pub fn new(_devtree: &DeviceTree, cpuid: usize) -> crate::Result<Self> {
        // TODO: Initialize x86_64 APIC (Advanced Programmable Interrupt Controller)
        // TODO: Get timebase frequency from CPUID or TSC
        let timebase_frequency = 1_000_000_000; // 1 GHz placeholder

        let tick_duration = Duration::from_nanos(NANOS_PER_SEC / timebase_frequency);

        // TODO: Implement x86_64 timer reading (TSC, HPET, etc.)
        let clock = Clock::new(tick_duration, || {
            // Placeholder: read TSC (Time Stamp Counter)
            unsafe {
                let low: u32;
                let high: u32;
                core::arch::asm!("rdtsc", out("eax") low, out("edx") high);
                Ticks(((high as u64) << 32) | (low as u64))
            }
        });

        // TODO: Initialize x86_64 CPU from device tree or ACPI tables
        Ok(Self {
            id: cpuid,
            clock: clock,
        })
    }

    pub fn interrupt_controller(&self) -> core::cell::RefMut<'_, dyn InterruptController> {
        todo!();
    }
}
