use crate::ensure;
use crate::error::Error;
use core::range::Range;
use mmu::arch::PAGE_SIZE;
use mmu::PhysicalAddress;

#[derive(Debug)]
pub struct WiredVmo {
    range: Range<PhysicalAddress>,
}

impl WiredVmo {
    pub fn new(range: Range<PhysicalAddress>) -> Self {
        Self { range }
    }

    pub(super) fn lookup_contiguous(
        &self,
        range: Range<usize>,
    ) -> crate::Result<Range<PhysicalAddress>> {
        ensure!(
            range.start % PAGE_SIZE == 0,
            Error::InvalidArgument,
            "range is not PAGE_SIZE aligned"
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
