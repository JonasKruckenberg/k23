// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::vm::Error;
use crate::vm::address::VirtualAddress;
use core::range::Range;
use core::{cmp, mem};

#[must_use]
pub struct Flush {
    asid: usize,
    range: Option<Range<VirtualAddress>>,
}

impl Drop for Flush {
    fn drop(&mut self) {
        if self.range.is_some() {
            log::error!("dropped Flush without calling ignore/flush");
        }
    }
}

impl Flush {
    pub fn empty(asid: usize) -> Self {
        Self { asid, range: None }
    }

    pub fn new(asid: usize, range: Range<VirtualAddress>) -> Self {
        Self {
            asid,
            range: Some(range),
        }
    }

    pub fn range(&self) -> Option<&Range<VirtualAddress>> {
        self.range.as_ref()
    }

    /// Flush the range of virtual addresses from the TLB.
    ///
    /// # Errors
    ///
    /// Returns an error if the range could not be flushed due to an underlying hardware error.
    pub fn flush(mut self) -> Result<(), Error> {
        log::trace!("flushing range {:?}", self.range);
        if let Some(range) = self.range.take() {
            arch::invalidate_range(self.asid, range) ?;
        } else {
            log::warn!("attempted to flush empty range, ignoring");
        }

        Ok(())
    }

    /// # Safety
    ///
    /// Not flushing after mutating the page translation tables will likely lead to unintended
    /// consequences such as inconsistent views of the address space between different harts.
    ///
    /// You should only call this if you know what you're doing.
    pub unsafe fn ignore(self) {
        mem::forget(self);
    }

    /// Extend the range to include the given range.
    ///
    /// # Errors
    ///
    /// Returns an error if the given ASID does not match the ASID of this `Flush`.
    pub fn extend_range(&mut self, asid: usize, other: Range<VirtualAddress>) -> Result<(), Error> {
        if self.asid == asid {
            if let Some(this) = self.range.take() {
                self.range = Some(Range {
                    start: cmp::min(this.start, other.start),
                    end: cmp::max(this.end, other.end),
                });
            } else {
                self.range = Some(other);
            }

            Ok(())
        } else {
            Err(Error::AddressSpaceMismatch {
                expected: self.asid,
                found: asid,
            })
        }
    }
}
