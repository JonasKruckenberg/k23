// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::error::Error;
use crate::vm::address::PhysicalAddress;
use crate::{arch, ensure};
use core::range::Range;

#[derive(Debug)]
pub struct WiredVmo {
    range: Range<PhysicalAddress>,
}

impl WiredVmo {
    pub fn new(range: Range<PhysicalAddress>) -> Self {
        Self { range }
    }

    pub fn lookup_contiguous(&self, range: Range<usize>) -> crate::Result<Range<PhysicalAddress>> {
        ensure!(
            range.start % arch::PAGE_SIZE == 0,
            Error::InvalidArgument,
            "range is not arch::PAGE_SIZE aligned"
        );
        let start = self.range.start.checked_add(range.start).unwrap();
        let end = self.range.start.checked_add(range.end).unwrap();

        ensure!(
            self.range.start <= start && self.range.end >= end,
            Error::AccessDenied,
            "requested range is out of bounds"
        );

        Ok(Range::from(start..end))
    }
}
