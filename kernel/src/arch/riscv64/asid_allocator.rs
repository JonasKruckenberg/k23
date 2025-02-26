// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use riscv::satp;
use sync::OnceLock;

// FIXME: A OnceLock to store a u16? yikes
static MAX_ASID: OnceLock<u16> = OnceLock::new();

pub fn init() {
    // Determine the number of supported ASID bits. The ASID is a "WARL" (Write Any Values, Reads Legal Values)
    // so we can write all 1s to and see which ones "stick".
    // Safety: register access
    unsafe {
        let orig = satp::read();
        satp::set(orig.mode(), 0xFFFF, orig.ppn());
        let max_asid = satp::read().asid();
        satp::set(orig.mode(), orig.asid(), orig.ppn());

        tracing::trace!("supported ASID bits: {} {max_asid}", max_asid.count_ones());
        MAX_ASID.get_or_init(|| max_asid);
    }
}

pub struct AsidAllocator {
    bitmap: Vec<u8>,
    last: u16,
}

impl fmt::Debug for AsidAllocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsidAllocator")
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

impl AsidAllocator {
    pub fn new() -> Self {
        let max_asid = *MAX_ASID.get().unwrap();
        let bitmap_size = (max_asid as usize + 1) / 8;

        Self {
            bitmap: vec![0; bitmap_size],
            last: 2,
        }
    }

    pub fn alloc(&mut self) -> Option<u16> {
        for asid in self.last + 1..u16::MAX {
            if !self.is_set(asid) {
                self.set(asid);
                return Some(asid);
            }
        }
        None
    }

    pub fn free(&mut self, asid: u16) {
        debug_assert!(self.is_set(asid));
        self.unset(asid);
    }

    fn is_set(&self, index: u16) -> bool {
        let byte = self.bitmap[index as usize / 8];
        (byte & 1 << (index % 8)) != 0
    }
    fn set(&mut self, index: u16) {
        self.bitmap[index as usize / 8] |= 1 << (index % 8);
    }
    fn unset(&mut self, index: u16) {
        self.bitmap[index as usize / 8] &= !(1 << (index % 8));
    }
}
