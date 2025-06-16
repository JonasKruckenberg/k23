// Claude generated code
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::device_tree::DeviceTree;

#[derive(Debug)]
pub struct Cpu {
    pub id: usize,
    pub clock: Clock,
}

impl Cpu {
    pub fn new(_devtree: &DeviceTree, cpuid: usize) -> crate::Result<Self> {
        let tick_duration = Duration::from_nanos(NANOS_PER_SEC / timebase_frequency);
        let clock = Clock::new(tick_duration, || Ticks(riscv::register::time::read64()));

        // TODO: Initialize x86_64 CPU from device tree or ACPI tables
        Ok(Self {
            id: cpuid,
            clock: clock,
        })
    }
}
