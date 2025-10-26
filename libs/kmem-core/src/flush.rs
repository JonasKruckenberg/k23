// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::mem;
use core::ops::Range;

use arrayvec::ArrayVec;

use crate::{Arch, VirtualAddress};

pub enum Flush<const CAP: usize = 16> {
    Ranges(ArrayVec<Range<VirtualAddress>, CAP>),
    All,
}

impl Default for Flush {
    fn default() -> Self {
        Self::new()
    }
}

impl Flush {
    pub const fn new() -> Self {
        Self::Ranges(ArrayVec::new())
    }

    /// Flush the range of virtual addresses from the TLB.
    pub fn flush<A>(self, arch: &A)
    where
        A: Arch,
    {
        match self {
            Flush::Ranges(ranges) => {
                for range in ranges {
                    log::trace!("flushing range {range:?}");
                    arch.fence(range);
                }
            }
            Flush::All => {
                log::trace!("flushing entire address space");
                arch.fence_all();
            }
        }
    }

    /// # Safety
    ///
    /// Not flushing after mutating the page translation tables will likely lead to unintended
    /// consequences such as inconsistent views of the address space between different cpus.
    ///
    /// You should only call this if you know what you're doing.
    pub const unsafe fn ignore(self) {
        mem::forget(self);
    }

    pub fn invalidate(&mut self, range: Range<VirtualAddress>) {
        match self {
            Flush::Ranges(ranges) => {
                ranges.push(range);
            }
            Flush::All => {}
        }
    }

    pub fn invalidate_all(&mut self) {
        *self = Flush::All;
    }
}
