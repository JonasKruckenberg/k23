// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

extern crate std;

use crate::address_space::RawAddressSpace;

#[derive(Debug)]
pub struct TestAddressSpace<const PAGE_SIZE: usize, const ADDR_BITS: u32> {}

impl<const PAGE_SIZE: usize, const ADDR_BITS: u32> TestAddressSpace<PAGE_SIZE, ADDR_BITS> {
    pub const fn new() -> Self {
        Self {}
    }
}

unsafe impl<const PAGE_SIZE: usize, const ADDR_BITS: u32> RawAddressSpace
    for TestAddressSpace<PAGE_SIZE, ADDR_BITS>
{
    const PAGE_SIZE: usize = PAGE_SIZE;
    const VIRT_ADDR_BITS: u32 = ADDR_BITS;
}
