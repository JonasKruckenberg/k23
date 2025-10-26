// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::ops::Range;

use crate::address_space::VisitMut;
use crate::arch::{Arch, PageTableEntry as _};
use crate::flush::Flush;
use crate::frame_alloc::{AllocError, FrameAllocator};
use crate::{AddressRangeExt, MemoryAttributes, PageTableLevel, PhysicalAddress, VirtualAddress};

pub struct MapVisitor<'flush, F> {
    pub phys: PhysicalAddress,
    attributes: MemoryAttributes,
    frame_allocator: F,
    frame_layout: Layout,
    flush: &'flush mut Flush,
}

impl<'flush, F> MapVisitor<'flush, F> {
    pub const fn new(
        phys: PhysicalAddress,
        attributes: MemoryAttributes,
        frame_allocator: F,
        frame_layout: Layout,
        flush: &'flush mut Flush,
    ) -> Self {
        Self {
            phys,
            attributes,
            frame_allocator,
            frame_layout,
            flush,
        }
    }
}

impl<A: Arch, F: FrameAllocator> VisitMut<A> for MapVisitor<'_, F> {
    type Error = AllocError;

    fn visit_entry(
        &mut self,
        entry: &mut A::PageTableEntry,
        level: &PageTableLevel,
        range: Range<VirtualAddress>,
        arch: &A,
    ) -> Result<(), Self::Error> {
        debug_assert!(entry.is_vacant());
        debug_assert!(!entry.is_leaf() && !entry.is_table());

        if level.can_map(range.start, self.phys, range.len()) {
            *entry = A::PageTableEntry::new_leaf(self.phys, self.attributes);

            self.phys = self.phys.add(range.len());

            // TODO fence(modified pages, 0) if attributes includes GLOBAL
            // TODO we can omit the fence here and lazily change the mapping in the fault handler#
            self.flush.invalidate(range);
        } else {
            let frame = self
                .frame_allocator
                .allocate_contiguous_zeroed(self.frame_layout, arch)?;

            *entry = A::PageTableEntry::new_table(frame);

            // TODO fence(all pages, 0) if attributes includes GLOBAL
            self.flush.invalidate_all();
        }

        Ok(())
    }
}
