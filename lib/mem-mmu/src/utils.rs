// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;
use core::range::{Range, RangeInclusive, RangeInclusiveIter};

use mem_core::VirtualAddress;
use mem_core::arch::{Arch, PageTableLevel};

/// Splits `range` into the per-entry sub-ranges of one table at `level`.
///
/// `range` must lie within a single table at `level`: a `PageTableEntries`
/// indexes one table, and each caller consumes it against one `Table`. A
/// cross-table range cannot be represented and panics in debug builds.
///
/// # Panics
///
/// Panics in debug builds if the range crosses tables at the chosen level.
pub fn page_table_entries_for<A: Arch>(
    range: Range<VirtualAddress>,
    level: &PageTableLevel,
) -> PageTableEntries<A> {
    debug_assert!(range.start.is_canonical::<A>() && range.end.is_canonical::<A>());
    debug_assert!(
        {
            // `range` stays within one table at this level iff its first and last
            // byte differ only below the table's span. Masking drops the sign-
            // extension bits, so the whole-space root table always passes.
            let va_mask = (1usize << A::VIRTUAL_ADDRESS_BITS) - 1;
            let table_span = level.entries() as usize * level.page_size();
            ((range.start.get() ^ range.end.sub(1).get()) & va_mask) < table_span
        },
        "range {range:?} crosses a page-table boundary at this level (page size {})",
        level.page_size()
    );

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

        let slot_end = self
            .page_start
            .align_down(self.page_size)
            .saturating_add(self.page_size);
        let page_range = Range::from(self.page_start..slot_end.min(self.max));

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
