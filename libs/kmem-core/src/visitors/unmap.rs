// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::convert::Infallible;
use core::ops::Range;

use crate::address_space::VisitMut;
use crate::arch::{Arch, PageTableEntry as _};
use crate::flush::Flush;
use crate::frame_alloc::FrameAllocator;
use crate::table::{Table, marker};
use crate::{PageTableLevel, VirtualAddress};

pub struct UnmapVisitor<'flush, F> {
    frame_allocator: F,
    frame_layout: Layout,
    flush: &'flush mut Flush,
}

impl<'flush, F> UnmapVisitor<'flush, F> {
    pub const fn new(frame_allocator: F, frame_layout: Layout, flush: &'flush mut Flush) -> Self {
        Self {
            frame_allocator,
            frame_layout,
            flush,
        }
    }
}

impl<A: Arch, F: FrameAllocator> VisitMut<A> for UnmapVisitor<'_, F> {
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
            *entry = A::PageTableEntry::VACANT;

            // TODO fence(modified pages, 0) if attributes includes GLOBAL
            self.flush.invalidate(range);
        }

        Ok(())
    }

    fn after_subtable(
        &mut self,
        entry: &mut A::PageTableEntry,
        table: Table<A, marker::Mut<'_>>,
        _level: &PageTableLevel,
        _range: Range<VirtualAddress>,
        arch: &A,
    ) -> Result<(), Self::Error> {
        if table.is_empty(arch) {
            let frame = entry.address();

            *entry = A::PageTableEntry::VACANT;

            // Safety: tables are always allocated through the frame allocator, and are always
            // exactly one frame in size.
            unsafe {
                self.frame_allocator.deallocate(frame, self.frame_layout);
            }

            // TODO fence(all pages, 0) if attributes includes GLOBAL
            self.flush.invalidate_all();
        }

        Ok(())
    }
}
