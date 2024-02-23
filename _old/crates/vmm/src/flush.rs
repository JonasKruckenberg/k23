use crate::{Mode, VirtualAddress};
use core::fmt::Formatter;
use core::marker::PhantomData;
use core::ops::Range;
use core::{cmp, fmt, mem};

#[must_use]
pub struct Flush<M> {
    asid: usize,
    address_range: Option<Range<VirtualAddress>>,
    _m: PhantomData<M>,
}

impl<A> fmt::Debug for Flush<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flush")
            .field("asid", &self.asid)
            .field("arange", &self.address_range)
            .finish()
    }
}

impl<M: Mode> Flush<M> {
    pub fn empty(asid: usize) -> Self {
        Self {
            asid,
            address_range: None,
            _m: PhantomData,
        }
    }

    pub fn new(asid: usize, range: Range<VirtualAddress>) -> Self {
        Self {
            asid,
            address_range: Some(range),
            _m: PhantomData,
        }
    }

    pub fn flush(self) -> crate::Result<()> {
        if let Some(range) = self.address_range {
            M::invalidate_range(self.asid, range)
        } else {
            log::warn!("attempted to flush empty range, ignoring...");
            Ok(())
        }
    }

    pub unsafe fn ignore(self) {
        mem::forget(self);
    }

    pub(crate) fn extend_range(
        &mut self,
        range: Range<VirtualAddress>,
        asid: usize,
    ) -> crate::Result<()> {
        debug_assert!(self.asid == asid,);

        if let Some(this) = &mut self.address_range {
            this.start = cmp::min(this.start, range.start);

            this.end = cmp::max(this.end, range.end);
        } else {
            self.address_range = Some(range);
        }

        Ok(())
    }
}
