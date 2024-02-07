use crate::arch::Arch;
use crate::error::ensure;
use crate::{Error, VirtualAddress};
use core::fmt::Formatter;
use core::marker::PhantomData;
use core::ops::Range;
use core::{cmp, fmt, mem};

pub struct Flush<A> {
    address_space: usize,
    range: Option<Range<VirtualAddress>>,
    _m: PhantomData<A>,
}

impl<A> fmt::Debug for Flush<A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flush")
            .field("address_space", &self.address_space)
            .field("range", &self.range)
            .finish()
    }
}

impl<A: Arch> Flush<A> {
    pub fn empty(address_space: usize) -> Self {
        Self {
            address_space,
            range: None,
            _m: PhantomData,
        }
    }

    pub fn new(address_space: usize, range: Range<VirtualAddress>) -> Self {
        Self {
            address_space,
            range: Some(range),
            _m: PhantomData,
        }
    }

    pub fn flush(self) -> crate::Result<()> {
        if let Some(range) = self.range {
            A::invalidate_range(self.address_space, range)
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
        address_space: usize,
    ) -> crate::Result<()> {
        ensure!(
            self.address_space == address_space,
            Error::AddressSpaceMismatch {
                expected: self.address_space,
                found: address_space
            }
        );

        if let Some(this) = &mut self.range {
            this.start = cmp::min(this.start, range.start);

            this.end = cmp::max(this.end, range.end);
        } else {
            self.range = Some(range);
        }

        Ok(())
    }
}
