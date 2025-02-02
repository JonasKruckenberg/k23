// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::num::NonZero;

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
