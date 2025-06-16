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
}

impl Cpu {
    pub fn new(_devtree: &DeviceTree, cpuid: usize) -> crate::Result<Self> {
        // TODO: Initialize x86_64 CPU from device tree or ACPI tables
        Ok(Self { id: cpuid })
    }
}