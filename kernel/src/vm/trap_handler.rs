// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use mmu::VirtualAddress;
use crate::error::Error;
use crate::vm::{PageFaultFlags, KERNEL_ASPACE};

pub fn trap_handler(faulting_addr: usize, flags: PageFaultFlags) -> crate::Result<()> {
    let mut aspace = KERNEL_ASPACE.get().unwrap().lock();

    let addr = VirtualAddress::new(faulting_addr).ok_or(Error::AccessDenied)?;

    aspace.page_fault(addr, flags)
}