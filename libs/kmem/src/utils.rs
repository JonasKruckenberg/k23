use core::marker::PhantomData;
use core::ops::{Range, RangeInclusive};

use crate::arch::{Arch, PageTableLevel};
use crate::VirtualAddress;

pub(crate) fn page_table_entries_for<A: Arch>(
    range: Range<VirtualAddress>,
    level: &PageTableLevel,
) -> PageTableEntries<A> {
    PageTableEntries {
        iter: level.pte_index_of(range.start)..=level.pte_index_of(range.end.sub(1)),
        page_start: range.start,
        max: range.end,
        page_size: level.page_size(),
        _arch: PhantomData,
    }
}

#[derive(Debug)]
pub struct PageTableEntries<A> {
    iter: RangeInclusive<u16>,
    page_start: VirtualAddress,
    max: VirtualAddress,
    page_size: usize,
    _arch: PhantomData<A>,
}

impl<A: Arch> Iterator for PageTableEntries<A> {
    type Item = (u16, Range<VirtualAddress>);

    fn next(&mut self) -> Option<Self::Item> {
        let entry_index = self.iter.next()?;

        let page_range =
            self.page_start..self.page_start.saturating_add(self.page_size).min(self.max);

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
