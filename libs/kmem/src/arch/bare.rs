// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem;

use crate::arch::{Arch, PageTableLevel};
use crate::{PhysicalAddress, VirtualAddress};

pub struct Bare<A> {
    arch: A,
}

impl<A> Bare<A> {
    pub const fn new(arch: A) -> Self {
        Self { arch }
    }
}

impl<A: Arch> Arch for Bare<A> {
    const PAGE_SIZE: usize = A::PAGE_SIZE;
    const PAGE_TABLE_LEVELS: &'static [PageTableLevel] = A::PAGE_TABLE_LEVELS;
    type PageTableEntry = A::PageTableEntry;

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        // We want identity-translation, so we just transmute one into the other
        // Safety: addresses are just numbers.
        unsafe { mem::transmute(phys) }
    }

    unsafe fn active_table(&mut self) -> PhysicalAddress {
        unsafe { self.arch.active_table() }
    }

    unsafe fn set_active_table(&mut self, addr: PhysicalAddress) {
        unsafe { self.arch.set_active_table(addr) }
    }
}
