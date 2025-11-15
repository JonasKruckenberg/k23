// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ops::{Range, RangeInclusive};

use crate::{MemoryMode, PageTableLevel, VirtualAddress};

// TODO test

pub(crate) fn page_table_entries_for(
    range: Range<VirtualAddress>,
    level: &PageTableLevel,
    memory_mode: &'static MemoryMode,
) -> PageTableEntries {
    PageTableEntries {
        iter: level.pte_index_of(range.start)..=level.pte_index_of(range.end.sub(1)),
        page_start: range.start,
        max: range.end,
        page_size: level.page_size(),
        memory_mode,
    }
}

#[derive(Debug)]
pub struct PageTableEntries {
    iter: RangeInclusive<usize>,
    page_start: VirtualAddress,
    max: VirtualAddress,
    page_size: usize,
    memory_mode: &'static MemoryMode,
}

impl Iterator for PageTableEntries {
    type Item = (usize, Range<VirtualAddress>);

    fn next(&mut self) -> Option<Self::Item> {
        let entry_index = self.iter.next()?;

        let page_range =
            self.page_start..self.page_start.saturating_add(self.page_size).min(self.max);

        if page_range.is_empty() {
            return None;
        }

        self.page_start = self.memory_mode.canonicalize(page_range.end);

        Some((entry_index, page_range))
    }
}
