// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::range::{Range, RangeInclusive, RangeInclusiveIter};

use crate::VirtualAddress;
use crate::arch::{Arch, PageTableLevel};

// TODO: tests
//  - ensure this only returns in-bound indices
pub fn page_table_entries_for<A: Arch>(
    range: Range<VirtualAddress>,
    level: &PageTableLevel,
) -> PageTableEntries<A> {
    PageTableEntries {
        iter: RangeInclusive::from(
            level.pte_index_of(range.start)..=level.pte_index_of(range.end.sub(1)),
        )
        .into_iter(),
        page_start: range.start,
        max: range.end,
        page_size: level.page_size(),
        _arch: PhantomData,
    }
}

#[derive(Debug)]
pub struct PageTableEntries<A> {
    iter: RangeInclusiveIter<u16>,
    page_start: VirtualAddress,
    max: VirtualAddress,
    page_size: usize,
    _arch: PhantomData<A>,
}

impl<A: Arch> Iterator for PageTableEntries<A> {
    type Item = (u16, Range<VirtualAddress>);

    fn next(&mut self) -> Option<Self::Item> {
        let entry_index = self.iter.next()?;

        let page_range = Range::from(
            self.page_start..self.page_start.saturating_add(self.page_size).min(self.max),
        );

        if page_range.is_empty() {
            return None;
        }

        self.page_start = page_range.end.canonicalize::<A>();

        Some((entry_index, page_range))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}
