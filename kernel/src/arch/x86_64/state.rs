// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::device::cpu::Cpu;

#[derive(Debug)]
pub struct Global {}

#[derive(Debug)]
pub struct CpuLocal {
    pub cpu: Cpu,
}
