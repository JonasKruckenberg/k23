use crate::{Error, Mode, VirtualAddress};
use core::marker::PhantomData;
use core::ops::Range;
use core::{cmp, mem};

pub struct Flush<M> {
    asid: usize,
    range: Range<VirtualAddress>,
    _m: PhantomData<M>,
}

impl<M: Mode> Flush<M> {
    pub fn empty(asid: usize) -> Self {
        Self {
            asid,
            range: unsafe { VirtualAddress::new(0)..VirtualAddress::new(0) },
            _m: PhantomData,
        }
    }

    pub fn new(asid: usize, range: Range<VirtualAddress>) -> Self {
        Self {
            asid,
            range,
            _m: PhantomData,
        }
    }

    pub fn flush(self) -> crate::Result<()> {
        if self.range.start == self.range.end {
            log::warn!("attempted to flush empty range, ignoring");
        } else {
            M::invalidate_range(self.asid, self.range)?;
        }

        Ok(())
    }

    pub unsafe fn ignore(self) {
        mem::forget(self);
    }

    pub fn extend_range(&mut self, asid: usize, range: Range<VirtualAddress>) -> crate::Result<()> {
        if self.asid == asid {
            self.range.start = cmp::min(self.range.start, range.start);
            self.range.end = cmp::max(self.range.start, range.end);
            Ok(())
        } else {
            Err(Error::AddressSpaceMismatch { expected: self.asid, found: asid})
        }
    }
}
