use crate::{Error, Mode, VirtualAddress};
use core::marker::PhantomData;
use core::ops::Range;
use core::{cmp, mem};

pub struct Flush<M> {
    asid: usize,
    range: Option<Range<VirtualAddress>>,
    _m: PhantomData<M>,
}

impl<M: Mode> Flush<M> {
    pub fn empty(asid: usize) -> Self {
        Self {
            asid,
            range: None,
            _m: PhantomData,
        }
    }

    pub fn new(asid: usize, range: Range<VirtualAddress>) -> Self {
        Self {
            asid,
            range: Some(range),
            _m: PhantomData,
        }
    }

    pub fn flush(self) -> crate::Result<()> {
        log::trace!("flushing range {:?}", self.range);
        if let Some(range) = self.range {
            M::invalidate_range(self.asid, range)?;
        } else {
            log::warn!("attempted to flush empty range, ignoring");
        }

        Ok(())
    }

    pub unsafe fn ignore(self) {
        mem::forget(self);
    }

    pub fn extend_range(&mut self, asid: usize, other: Range<VirtualAddress>) -> crate::Result<()> {
        if self.asid == asid {
            if let Some(this) = self.range.take() {
                self.range = Some(Range {
                    start: cmp::min(this.start, other.start),
                    end: cmp::max(this.start, other.end),
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
