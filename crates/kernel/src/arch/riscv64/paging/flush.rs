use crate::paging::VirtualAddress;
use core::ops::Range;
use core::{cmp, mem};

pub struct Flush {
    range: Range<VirtualAddress>,
}

impl Flush {
    pub fn empty() -> Self {
        Self {
            range: unsafe { VirtualAddress::new(0)..VirtualAddress::new(0) },
        }
    }

    pub fn new(range: Range<VirtualAddress>) -> Self {
        Self { range }
    }

    pub fn flush(self) -> crate::Result<()> {
        // TODO check if this is necessary
        sbicall::rfence::sfence_vma(
            0,
            -1isize as usize,
            self.range.start.as_raw(),
            self.range.end.as_raw(),
        )?;

        Ok(())
    }

    pub unsafe fn ignore(self) {
        mem::forget(self);
    }

    pub fn join(&mut self, other: Flush) {
        self.range.start = cmp::min(self.range.start, other.range.start);
        self.range.end = cmp::max(self.range.start, other.range.end);
    }
}
