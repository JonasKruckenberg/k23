// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::convert::Infallible;
use core::ops::Range;

use crate::address_space::VisitMut;
use crate::arch::{Arch, PageTableEntry as _};
use crate::flush::Flush;
use crate::{MemoryAttributes, PageTableLevel, VirtualAddress};

pub struct SetAttributesVisitor<'flush> {
    attributes: MemoryAttributes,
    flush: &'flush mut Flush,
}

impl<'flush> SetAttributesVisitor<'flush> {
    pub const fn new(attributes: MemoryAttributes, flush: &'flush mut Flush) -> Self {
        Self { attributes, flush }
    }
}

impl<A: Arch> VisitMut<A> for SetAttributesVisitor<'_> {
    type Error = Infallible;

    fn visit_entry(
        &mut self,
        entry: &mut A::PageTableEntry,
        _level: &PageTableLevel,
        range: Range<VirtualAddress>,
        _arch: &A,
    ) -> Result<(), Self::Error> {
        debug_assert!(!entry.is_vacant());

        if entry.is_leaf() {
            *entry = A::PageTableEntry::new_leaf(entry.address(), self.attributes);

            // TODO fence(modified pages, 0) if attributes includes GLOBAL
            // TODO we can omit the fence here IF the attributes are MORE PERMISSIVE than before and
            // lazily change the mapping in the fault handler
            self.flush.invalidate(range);
        }

        Ok(())
    }
}
